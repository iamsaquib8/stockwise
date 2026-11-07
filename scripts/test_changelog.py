import contextlib
import io
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import changelog as cl  # noqa: E402

SAMPLE = """# Changelog

## [Unreleased]

### Fixed
- Old entry left under Unreleased.

## [1.0.0] - 2026-01-01

### Added
- First release.

[Unreleased]: https://github.com/iamsaquib8/stockwise/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/iamsaquib8/stockwise/releases/tag/v1.0.0
"""


class Fragments(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        (self.root / "changelog.d").mkdir()
        (self.root / "CHANGELOG.md").write_text(SAMPLE)
        cl.FRAGMENTS = self.root / "changelog.d"
        cl.CHANGELOG = self.root / "CHANGELOG.md"

    def tearDown(self):
        self.tmp.cleanup()

    def frag(self, name, text):
        (self.root / "changelog.d" / name).write_text(text)

    def test_bad_name_and_content_are_rejected(self):
        self.frag("oops.md", "- x\n")
        self.frag("1.fixed.md", "no bullet\n")
        self.frag("2.fixed.md", "- one\n- two\n")
        with self.assertRaises(cl.FragmentError) as e:
            cl.read_fragments(cl.FRAGMENTS)
        msg = str(e.exception)
        self.assertIn("oops.md", msg)
        self.assertIn("1.fixed.md", msg)
        self.assertIn("2.fixed.md", msg)

    def test_release_folds_fragments_and_unreleased(self):
        self.frag("10.fixed.md", "- `tax in` fixed ([#10](x)).\n")
        self.frag("11.added.md", "- `new` command ([#11](x)).\n  Second line.\n")
        self.frag("12.changed.md", "- `quote` shows more ([#12](x)).\n")
        self.assertEqual(cl.main(["release", "1.1.0", "--date", "2026-02-02"]), 0)
        text = (self.root / "CHANGELOG.md").read_text()
        head = text.index("## [1.1.0] - 2026-02-02")
        old = text.index("## [1.0.0]")
        section = text[head:old]
        self.assertLess(section.index("### Added"), section.index("### Changed"))
        self.assertLess(section.index("### Changed"), section.index("### Fixed"))
        self.assertIn("Old entry left under Unreleased.", section)
        self.assertIn("- `tax in` fixed", section)
        self.assertIn("  Second line.", section)
        self.assertIn("## [Unreleased]\n\n## [1.1.0]", text)
        self.assertIn("[Unreleased]: https://github.com/iamsaquib8/stockwise/compare/v1.1.0...HEAD", text)
        self.assertIn("[1.1.0]: https://github.com/iamsaquib8/stockwise/compare/v1.0.0...v1.1.0", text)
        self.assertEqual([p.name for p in cl.FRAGMENTS.glob("*.md")], [])

    def test_preview_notes_have_the_entries_and_no_heading(self):
        self.frag("20.performance.md", "- `quote` is faster ([#20](x)).\n")
        self.frag("21.fixed.md", "- `tax in` fixed ([#21](x)).\n")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(cl.main(["preview", "--notes"]), 0)
        text = out.getvalue()
        self.assertNotIn("## [Unreleased]", text)
        self.assertLess(text.index("### Fixed"), text.index("### Performance"))
        self.assertIn("Old entry left under Unreleased.", text)
        self.assertIn("`quote` is faster", text)
        # Nothing pending: no output at all, so the caller can fall back.
        (self.root / "CHANGELOG.md").write_text(SAMPLE.replace("### Fixed\n- Old entry left under Unreleased.\n", ""))
        for f in cl.FRAGMENTS.glob("*.md"):
            f.unlink()
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cl.main(["preview", "--notes"])
        self.assertEqual(out.getvalue(), "")

    def test_release_refuses_empty_and_duplicate(self):
        (self.root / "CHANGELOG.md").write_text(SAMPLE.replace("### Fixed\n- Old entry left under Unreleased.\n", ""))
        self.assertEqual(cl.main(["release", "1.1.0"]), 2)
        (self.root / "CHANGELOG.md").write_text(SAMPLE)
        self.assertEqual(cl.main(["release", "1.0.0"]), 2)
        self.assertEqual(cl.main(["release", "one"]), 2)


if __name__ == "__main__":
    unittest.main()
