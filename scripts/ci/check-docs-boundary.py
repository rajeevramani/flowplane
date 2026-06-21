#!/usr/bin/env python3
"""Enforce the docs cardinal rule (#116): user docs under docs/ must stand alone.

Fails on any link from a `docs/**/*.md` page into `../internal/` or `../spec/`
that is NOT one of the allowed carve-outs (see docs/README.md "Enforcement (CI)"):

  1. docs/README.md (the index) may link anywhere; and a link whose target is a
     bucket index (internal/README.md, spec/README.md) is allowed from any page.
  2. A docs/concepts/ page may link into spec/ (explanation bridge).
  3. A spec/ link under a "## Further reading" / "Design references" heading
     (optional background) is allowed.

Everything else — any internal/ link from a task page, and required-reading spec/
links in task steps — fails.

Run:  scripts/ci/check-docs-boundary.py            # check the tree
      scripts/ci/check-docs-boundary.py --self-test # run the parser self-checks
"""
from __future__ import annotations

import os
import re
import sys

# scripts/ci/check-docs-boundary.py -> repo root
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# Inline links `[text](target)`.
LINK_RE = re.compile(r"\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+[^)]*)?\)")
# Reference definition `[label]: target` -> (label, target).
REFDEF_RE = re.compile(r"^\s*\[([^\]]+)\]:\s*<?([^\s>]+)>?")
# Reference usage `[text][id]` / collapsed `[text][]` -> (text, id).
REFUSE_RE = re.compile(r"\[([^\]]+)\]\[([^\]]*)\]")
# Shortcut usage `[id]`: not preceded by `]` (so it is not the id half of `[text][id]`) and not
# followed by `(`, `[`, or `:` (so it is neither inline nor a definition).
SHORTCUT_RE = re.compile(r"(?<!\])\[([^\]]+)\](?![\(\[:])")
# HTML `href="target"` — boundary so `data-href=` / `xhref=` do not match.
HREF_RE = re.compile(r"""(?<![\w-])href\s*=\s*["']([^"']+)["']""", re.IGNORECASE)
HEADING_RE = re.compile(r"^#{1,6}\s+(.*?)\s*#*\s*$")
FENCE_RE = re.compile(r"^\s*(```|~~~)")
# Exact (normalized) headings that make a following spec/ link optional background.
FURTHER_READING = frozenset({"further reading", "design references", "design reference"})


def _collect_defs(lines: list[str]) -> dict[str, str]:
    """Map reference-definition id -> target (skipping fenced code)."""
    defs: dict[str, str] = {}
    in_fence = False
    for line in lines:
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        m = REFDEF_RE.match(line)
        if m:
            defs[m.group(1).lower()] = m.group(2)
    return defs


def _targets(line: str, defs: dict[str, str]) -> list[str]:
    """Every link target *used* on this line (so the section rule applies at the usage point)."""
    out = LINK_RE.findall(line) + HREF_RE.findall(line)
    for text, ref in REFUSE_RE.findall(line):
        target = defs.get((ref or text).lower())
        if target:
            out.append(target)
    for text in SHORTCUT_RE.findall(line):
        target = defs.get(text.lower())
        if target:
            out.append(target)
    return out


def find_violations(path: str, text: str) -> list[tuple[int, str, str]]:
    """Return (line_no, target, reason) for each disallowed internal/spec link."""
    rel_path = os.path.relpath(path, REPO_ROOT).replace(os.sep, "/")
    is_index = rel_path == "docs/README.md"
    is_concepts = rel_path.startswith("docs/concepts/")
    file_dir = os.path.dirname(path)

    lines = text.splitlines()
    defs = _collect_defs(lines)

    violations: list[tuple[int, str, str]] = []
    section = ""
    in_fence = False
    for line_no, line in enumerate(lines, start=1):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        heading = HEADING_RE.match(line)
        if heading:
            # Normalize to letters+spaces so "## Further reading" / "Further Reading:" match,
            # but "Not further reading" / "Further reading required setup" do NOT.
            section = re.sub(r"[^a-z ]", "", heading.group(1).lower()).strip()
        for raw in _targets(line, defs):
            target = raw.split("#", 1)[0].strip()
            if not target or "://" in target or target.startswith("#"):
                continue
            resolved = os.path.normpath(os.path.join(file_dir, target))
            resolved = os.path.relpath(resolved, REPO_ROOT).replace(os.sep, "/")
            bucket = (
                "internal"
                if resolved == "internal" or resolved.startswith("internal/")
                else "spec"
                if resolved == "spec" or resolved.startswith("spec/")
                else None
            )
            if bucket is None:
                continue
            # --- carve-outs ---
            if is_index:
                continue
            if resolved in ("internal/README.md", "spec/README.md"):
                continue  # bucket index, allowed from anywhere
            if bucket == "spec" and is_concepts:
                continue  # explanation bridge
            if bucket == "spec" and section in FURTHER_READING:
                continue  # optional background
            reason = (
                f"required-reading link into {bucket}/ from a user-doc page"
                if bucket == "internal"
                else f"spec/ link outside concepts/ and outside a 'Further reading' section"
            )
            violations.append((line_no, target, reason))
    return violations


def check_tree() -> int:
    docs_dir = os.path.join(REPO_ROOT, "docs")
    failures = 0
    for root, _dirs, files in os.walk(docs_dir):
        for name in sorted(files):
            if not name.endswith(".md"):
                continue
            path = os.path.join(root, name)
            with open(path, encoding="utf-8") as fh:
                text = fh.read()
            for line_no, target, reason in find_violations(path, text):
                rel = os.path.relpath(path, REPO_ROOT)
                print(f"{rel}:{line_no}: {reason} -> {target}")
                failures += 1
    if failures:
        print(f"\n{failures} docs-boundary violation(s). See docs/README.md "
              f"'Enforcement (CI)'.", file=sys.stderr)
        return 1
    print("docs boundary OK: no disallowed internal/ or spec/ links.")
    return 0


def self_test() -> int:
    def v(path, text):
        return [(t, r) for _l, t, r in find_violations(os.path.join(REPO_ROOT, path), text)]

    # internal link from a how-to page -> violation
    assert v("docs/how-to/x.md", "see [setup](../../internal/dev.md)"), "internal link must fail"
    # bucket index allowed from anywhere
    assert not v("docs/how-to/x.md", "see [internal](../../internal/README.md)")
    # spec link from a how-to (no Further reading section) -> violation
    assert v("docs/how-to/x.md", "see [design](../../spec/04.md)"), "bare spec link must fail"
    # spec link under Further reading -> allowed
    assert not v("docs/how-to/x.md", "## Further reading\n[design](../../spec/04.md)")
    # a heading that merely contains the phrase must NOT exempt (finding #2)
    assert v("docs/how-to/x.md", "## Not further reading\n[design](../../spec/04.md)")
    assert v("docs/how-to/x.md", "## Further reading required setup\n[d](../../spec/04.md)")
    # reference-style usage is parsed; the section rule applies at the USAGE line, not the
    # definition (round-2 finding #1). Usage in a normal section + def parked under Further
    # reading must STILL fail.
    assert v("docs/how-to/x.md", "see [design][d]\n## Further reading\n[d]: ../../spec/04.md")
    # a full reference usage is reported exactly once, not doubled by the shortcut rule
    assert len(v("docs/how-to/x.md", "see [design][d]\n[d]: ../../spec/04.md")) == 1
    # but a reference usage that itself sits under Further reading is allowed
    assert not v("docs/how-to/x.md", "## Further reading\nsee [design][d]\n\n[d]: ../../spec/04.md")
    # internal reference usage always fails regardless of section
    assert v("docs/how-to/x.md", "## Further reading\n[dev][d]\n[d]: ../../internal/dev.md")
    # HTML href links are parsed; data-href / xhref must NOT match (round-2 finding #2)
    assert v("docs/how-to/x.md", '<a href="../../internal/dev.md">dev</a>')
    assert not v("docs/how-to/x.md", '<span data-href="../../internal/dev.md">x</span>')
    # concepts -> spec allowed; concepts -> internal still fails
    assert not v("docs/concepts/y.md", "[design](../../spec/04.md)")
    assert v("docs/concepts/y.md", "[dev](../../internal/dev.md)"), "concepts->internal must fail"
    # index file exempt
    assert not v("docs/README.md", "[dev](../internal/dev.md) [spec](../spec/04.md)")
    # external + anchor-only ignored
    assert not v("docs/how-to/x.md", "[ext](https://x/internal/y) [a](#internal)")
    # link inside a fenced code block ignored
    assert not v("docs/how-to/x.md", "```\n[x](../../internal/dev.md)\n```")
    # sibling links (not into internal/spec) ignored
    assert not v("docs/how-to/x.md", "[other](../reference/cli.md)")
    print("self-test OK")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(check_tree())
