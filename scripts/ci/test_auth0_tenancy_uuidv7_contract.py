#!/usr/bin/env python3
"""Black-box regression contract for the Auth0 tenancy acceptance CLI.

The harness implementation is intentionally not imported or inspected.  This
covers only its committed command-line contract.  Its public summary confirms
that fail-closed mutations ran, but it does not identify a UUIDv7 mutation, so
the UUIDv7-specific subcase cannot be independently proven from CLI output.
"""

from __future__ import annotations

import re
import subprocess
import sys
import unittest
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
HARNESS = REPOSITORY_ROOT / "scripts" / "ci" / "check-auth0-tenancy-acceptance.py"
SUMMARY_PATTERN = re.compile(
    r"^auth0 tenancy acceptance self-test: PASS "
    r"\((?P<scenarios>[1-9]\d*) scenarios, "
    r"(?P<mutations>[1-9]\d*) fail-closed mutations\)$"
)


class Auth0TenancyUuidV7ContractTest(unittest.TestCase):
    def test_self_test_exits_successfully_and_reports_fail_closed_mutations(self) -> None:
        result = subprocess.run(
            [sys.executable, str(HARNESS), "--self-test"],
            cwd=REPOSITORY_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )

        self.assertEqual(result.returncode, 0, msg=result.stderr or result.stdout)
        summary = result.stdout.strip()
        self.assertIsNotNone(SUMMARY_PATTERN.fullmatch(summary), msg=summary)
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
