#!/usr/bin/env python3
"""Reject internal planning jargon in public Markdown docs.

The public docs must stand alone for readers who have not seen Flowplane's
issue tracker, Beads graph, vault, or implementation slice plan. This check is
intentionally scoped to tracked public Markdown only:

  * README.md
  * docs/**/*.md
  * deploy/aws/**/*.md, including the top-level deploy/aws/README.md

Run:  scripts/ci/check-docs-jargon.py             # check the tree
      scripts/ci/check-docs-jargon.py --self-test # run parser self-checks
"""
from __future__ import annotations

import os
import re
import subprocess
import sys
from dataclasses import dataclass

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
FENCE_RE = re.compile(r"^\s*(```|~~~)")


@dataclass(frozen=True)
class Marker:
    name: str
    pattern: re.Pattern[str]


MARKERS = (
    Marker("internal slice label", re.compile(r"\bS(?!3\b)[0-9]+[a-z]*\b")),
    Marker("Beads issue id", re.compile(r"\bfpv2-[A-Za-z0-9.-]+\b")),
    Marker("Beads planning term", re.compile(r"\bbeads?\b", re.IGNORECASE)),
)


def _git_ls_files() -> list[str]:
    result = subprocess.run(
        ["git", "ls-files"],
        cwd=REPO_ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return result.stdout.splitlines()


def public_markdown_files(paths: list[str] | None = None) -> list[str]:
    tracked = paths if paths is not None else _git_ls_files()
    out: list[str] = []
    for path in tracked:
        normalized = path.replace(os.sep, "/")
        if normalized == "README.md":
            out.append(normalized)
        elif normalized.startswith("docs/") and normalized.endswith(".md"):
            out.append(normalized)
        elif normalized.startswith("deploy/aws/") and normalized.endswith(".md"):
            out.append(normalized)
    return sorted(out)


def find_violations(path: str, text: str) -> list[tuple[int, str, str]]:
    violations: list[tuple[int, str, str]] = []
    in_fence = False
    for line_no, line in enumerate(text.splitlines(), start=1):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        for marker in MARKERS:
            match = marker.pattern.search(line)
            if match:
                violations.append((line_no, marker.name, match.group(0)))
    return violations


def check_tree() -> int:
    failures = 0
    for rel_path in public_markdown_files():
        path = os.path.join(REPO_ROOT, rel_path)
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        for line_no, name, token in find_violations(rel_path, text):
            print(f"{rel_path}:{line_no}: {name} -> {token}")
            failures += 1
    if failures:
        print(f"\n{failures} docs-jargon violation(s). Public docs must not expose internal planning labels.", file=sys.stderr)
        return 1
    print("docs jargon OK: no internal planning markers in public Markdown.")
    return 0


def self_test() -> int:
    sample_paths = [
        "README.md",
        "docs/reference/cli.md",
        "deploy/aws/README.md",
        "deploy/aws/nested/example.md",
        "deploy/gcp/README.md",
        "internal/README.md",
        "spec/README.md",
        "CHANGELOG.md",
    ]
    assert public_markdown_files(sample_paths) == [
        "README.md",
        "deploy/aws/README.md",
        "deploy/aws/nested/example.md",
        "docs/reference/cli.md",
    ]
    assert find_violations("docs/x.md", "xDS from S5")
    assert find_violations("docs/x.md", "new S12b metrics")
    assert not find_violations("docs/x.md", "AWS S3 bucket")
    assert find_violations("docs/x.md", "tracked by fpv2-214")
    assert find_violations("docs/x.md", "the Beads graph")
    assert not find_violations("docs/x.md", "```\nxDS from S5\n```")
    print("self-test OK")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(check_tree())
