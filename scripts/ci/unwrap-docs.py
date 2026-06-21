#!/usr/bin/env python3
"""Unwrap hard-wrapped prose in Markdown so renderers that treat single newlines as line breaks
(this project's doc site) show flowing text instead of mid-sentence breaks.

Joins each paragraph and each list item onto a single line, collapsing the wrap whitespace. Leaves
structure intact: blank lines, ATX headings (#), tables (|), fenced code (``` / ~~~), blockquotes
(>), indented code (4+ spaces), and any line ending in two spaces (an intentional hard break). A
list item's wrapped continuation lines are folded into the item; nesting indentation is preserved.

Usage:
  scripts/ci/unwrap-docs.py --check PATH...   # exit 1 if any file would change (no writes)
  scripts/ci/unwrap-docs.py         PATH...   # rewrite in place
"""
from __future__ import annotations

import re
import sys

FENCE = re.compile(r"^\s*(```|~~~)")
HEADING = re.compile(r"^\#{1,6}\s")
TABLE = re.compile(r"^\s*\|")
BLOCKQUOTE = re.compile(r"^\s*>")
HTML = re.compile(r"^\s*<")
INDENTED_CODE = re.compile(r"^ {4,}\S")
LIST_ITEM = re.compile(r"^(?P<indent>\s*)(?P<marker>[-*+]|\d+[.)])\s+(?P<body>.*)$")


def _is_block_start(line: str) -> bool:
    """A line that begins (or is) its own block and must never be folded into a paragraph."""
    return bool(
        line.strip() == ""
        or HEADING.match(line)
        or TABLE.match(line)
        or BLOCKQUOTE.match(line)
        or HTML.match(line)
        or INDENTED_CODE.match(line)
    )


def unwrap(text: str) -> str:
    lines = text.split("\n")
    out: list[str] = []
    in_fence = False
    # An open accumulator is either a paragraph (prefix == "") or a list item (prefix == marker).
    buf: list[str] = []
    prefix: str | None = None  # None = nothing open; "" = paragraph; else the list-item prefix.

    def flush():
        nonlocal prefix
        if prefix is not None:
            joined = " ".join(s.strip() for s in buf if s.strip())
            out.append(f"{prefix}{joined}" if prefix else joined)
            buf.clear()
            prefix = None

    for line in lines:
        if FENCE.match(line):
            flush()
            in_fence = not in_fence
            out.append(line)
            continue
        if in_fence:
            out.append(line)
            continue

        item = LIST_ITEM.match(line)
        if item:
            flush()
            prefix = f"{item.group('indent')}{item.group('marker')} "
            buf.append(item.group("body"))
            continue

        if _is_block_start(line) or line.endswith("  "):
            flush()
            out.append(line)
            continue

        # Plain text line. It continues an open paragraph / list item, or opens a new paragraph —
        # preserving its leading indentation so an indented list-continuation paragraph stays
        # indented (i.e. stays inside its list item) rather than dedenting to column 0.
        if prefix is None:
            prefix = re.match(r"\s*", line).group()
        buf.append(line)

    flush()
    return "\n".join(out)


def main() -> int:
    args = sys.argv[1:]
    check = "--check" in args
    paths = [a for a in args if a != "--check"]
    changed = []
    for p in paths:
        with open(p, encoding="utf-8") as fh:
            original = fh.read()
        new = unwrap(original)
        if new != original:
            changed.append(p)
            if not check:
                with open(p, "w", encoding="utf-8") as fh:
                    fh.write(new)
    label = "would unwrap" if check else "unwrapped"
    for p in changed:
        print(f"{label}: {p}")
    return 1 if (check and changed) else 0


if __name__ == "__main__":
    sys.exit(main())
