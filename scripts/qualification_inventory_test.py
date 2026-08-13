#!/usr/bin/env python3
"""Regression tests for exact-artifact inventory capture."""

import importlib.util
import pathlib
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("qualification-inventory.py")
SPEC = importlib.util.spec_from_file_location("qualification_inventory", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class QualificationInventoryTests(unittest.TestCase):
    def test_markdown_commands_are_filtered_to_live_schema_paths(self):
        markdown = """
### `qualification`
| `--json` | global flag |
| `qualification inventory --input <PATH> --output-path <PATH>` | generate |
| `cluster list` | list clusters |
"""
        live = {
            "qualification",
            "qualification inventory",
            "qualification validate",
            "cluster",
            "cluster list",
        }
        self.assertEqual(
            MODULE.markdown_cli_commands(markdown, live),
            {"qualification", "qualification inventory", "cluster list"},
        )

    def test_single_declaration_or_code_surface_stays_incomplete(self):
        for surface in (
            "openapi",
            "cli_schema",
            "dashboard_routes",
            "cli_help",
            "stable_docs",
            "config",
        ):
            with self.subTest(surface=surface):
                classification, _ = MODULE.classify("example", {surface})
                self.assertEqual(classification, "incomplete")

    def test_support_requires_executable_and_independent_declaration(self):
        classification, _ = MODULE.classify(
            "cli:cluster list", {"cli_schema", "cli_help"}
        )
        self.assertEqual(classification, "supported-core")

    def test_exact_binary_is_the_only_single_surface_support_exception(self):
        classification, _ = MODULE.classify("binary:flowplane", {"binaries"})
        self.assertEqual(classification, "supported-core")


if __name__ == "__main__":
    unittest.main()
