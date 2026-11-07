#!/usr/bin/env python3
"""Tests for the pure helpers in site/build.py. Run: python3 site/test_build.py"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build  # noqa: E402

CHANGELOG = """# Changelog

## [Unreleased]

### Added
- Something new.

## [0.8.0] - 2026-11-03

## [0.7.0] - 2026-11-01
"""


class LatestRelease(unittest.TestCase):
    def test_skips_unreleased(self):
        self.assertEqual(build.latest_release(CHANGELOG), "0.8.0")

    def test_none_without_a_release(self):
        self.assertIsNone(build.latest_release("# Changelog\n\n## [Unreleased]\n"))

    def test_the_real_changelog_has_a_release(self):
        text = (build.ROOT / "CHANGELOG.md").read_text()
        self.assertRegex(build.latest_release(text) or "", r"^\d+\.\d+\.\d+$")


class ToolCount(unittest.TestCase):
    def test_read_from_the_ai_page(self):
        self.assertRegex(build.mcp_tool_count(), r"^\d+$")


class Banner(unittest.TestCase):
    PAGE = "<body><nav>menu</nav><main><h1>Install</h1></main></body>"

    def test_goes_first_in_main_and_names_the_release(self):
        out = build.add_banner(self.PAGE, "0.8.0")
        self.assertIn("<main><div class=\"docs-version-banner\"", out)
        self.assertIn("the latest release is", out)
        self.assertIn(">v0.8.0</a>", out)
        self.assertIn("releases/tag/v0.8.0", out)
        self.assertTrue(out.endswith("<h1>Install</h1></main></body>"))

    def test_added_once(self):
        once = build.add_banner(self.PAGE, "0.8.0")
        self.assertEqual(build.add_banner(once, "0.8.0"), once)

    def test_page_without_main_is_unchanged(self):
        self.assertEqual(build.add_banner("<p>x</p>", "0.8.0"), "<p>x</p>")


if __name__ == "__main__":
    unittest.main()
