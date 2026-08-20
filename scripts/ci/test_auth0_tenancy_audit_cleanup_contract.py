#!/usr/bin/env python3
"""Black-box contract for the Auth0 tenancy audit/cleanup self-test.

The harness implementation is intentionally neither imported nor inspected.
The public CLI output proves the aggregate self-test result and mutation count;
specific pre-authorization and cleanup subcases are proven internally by the
harness and are not independently identifiable from its public summary.
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
    r"(?P<mutations>[1-9]\d*) fail-closed mutations\)\n?$"
)


class Auth0TenancyAuditCleanupContractTest(unittest.TestCase):
    def test_self_test_reports_required_audit_cleanup_aggregate(self) -> None:
        result = subprocess.run(
            [sys.executable, str(HARNESS), "--self-test"],
            cwd=REPOSITORY_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )

        self.assertEqual(result.returncode, 0, msg=result.stderr or result.stdout)
        self.assertEqual(result.stderr, "")

        match = SUMMARY_PATTERN.fullmatch(result.stdout)
        self.assertIsNotNone(match, msg=result.stdout)
        assert match is not None
        self.assertEqual(int(match.group("scenarios")), 12)
        self.assertEqual(int(match.group("mutations")), 21)


if __name__ == "__main__":
    unittest.main()
