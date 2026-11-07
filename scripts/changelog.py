#!/usr/bin/env python3
"""Collect changelog fragments from changelog.d/ into CHANGELOG.md.

Usage:
    changelog.py check              validate fragment names and content
    changelog.py preview [--notes]  print the sections a release would add
                                    (--notes: only the entries, for release notes)
    changelog.py release VERSION [--date YYYY-MM-DD]
                                    cut VERSION: fold fragments and the
                                    [Unreleased] entries into a new section

See changelog.d/README.md. Standard library only.
"""

import argparse
import datetime
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
FRAGMENTS = ROOT / "changelog.d"
CHANGELOG = ROOT / "CHANGELOG.md"
REPO = "https://github.com/iamsaquib8/stockwise"

# Section order in a release, as in Keep a Changelog.
SECTIONS = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Performance", "Security"]
TYPES = {s.lower(): s for s in SECTIONS}
NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*\.(%s)\.md$" % "|".join(TYPES))


class FragmentError(Exception):
    pass


def read_fragments(directory=None):
    """Return {section: [entry text, ...]} from every fragment, sorted by name."""
    directory = directory or FRAGMENTS
    found = {s: [] for s in SECTIONS}
    errors = []
    for path in sorted(directory.glob("*.md")):
        if path.name == "README.md":
            continue
        match = NAME_RE.match(path.name)
        if not match:
            errors.append(
                f"{path.name}: name must be <issue-or-slug>.<type>.md with type in "
                f"{', '.join(TYPES)}"
            )
            continue
        text = path.read_text(encoding="utf-8").rstrip()
        if not text.startswith("- "):
            errors.append(f"{path.name}: the entry must start with '- '")
            continue
        if "\n- " in text:
            errors.append(f"{path.name}: one entry per file (found a second '- ' bullet)")
            continue
        if any(len(line) > 400 for line in text.splitlines()):
            errors.append(f"{path.name}: a line is longer than 400 characters; wrap it")
            continue
        found[TYPES[match.group(1)]].append(text)
    if errors:
        raise FragmentError("\n".join(errors))
    return found


def split_unreleased(changelog):
    """Return (before, unreleased_body, after) around the [Unreleased] block."""
    start = changelog.index("## [Unreleased]\n")
    body_start = start + len("## [Unreleased]\n")
    nxt = re.search(r"^## \[", changelog[body_start:], re.M)
    if not nxt:
        raise FragmentError("no released version after [Unreleased] in CHANGELOG.md")
    end = body_start + nxt.start()
    return changelog[:body_start], changelog[body_start:end], changelog[end:]


def parse_sections(body):
    """Split a '### Section' body into (intro, {section: entries text})."""
    intro, sections, current = [], {}, None
    for line in body.splitlines():
        head = re.match(r"^### (\w+)\s*$", line)
        if head:
            current = head.group(1)
            sections.setdefault(current, [])
        elif current is None:
            intro.append(line)
        else:
            sections[current].append(line)
    intro_text = re.sub(r"<!--.*?-->", "", "\n".join(intro), flags=re.S).strip("\n")
    return intro_text, {k: "\n".join(v).strip("\n") for k, v in sections.items()}


def render(version, date, intro, existing, fragments):
    out = [f"## [{version}] - {date}\n"]
    if intro:
        out.append(intro + "\n")
    names = SECTIONS + sorted(set(existing) - set(SECTIONS))
    for name in names:
        parts = [p for p in (existing.get(name, ""), "\n".join(fragments.get(name, []))) if p]
        if parts:
            out.append(f"### {name}\n" + "\n".join(parts) + "\n")
    return "\n".join(out)


def previous_version(changelog):
    match = re.search(r"^\[Unreleased\]: .*/compare/v([0-9][^.\s]*(?:\.[0-9]+)*)\.\.\.HEAD$", changelog, re.M)
    if not match:
        raise FragmentError("cannot find the [Unreleased] compare link at the bottom of CHANGELOG.md")
    return match.group(1)


def cmd_check(_args):
    fragments = read_fragments()
    n = sum(len(v) for v in fragments.values())
    print(f"changelog.d: {n} fragment(s) OK")
    return 0


def cmd_preview(args):
    fragments = read_fragments()
    changelog = CHANGELOG.read_text(encoding="utf-8")
    _, body, _ = split_unreleased(changelog)
    intro, existing = parse_sections(body)
    text = render("Unreleased", "next", intro, existing, fragments)
    if args.notes:
        # Drop the "## [Unreleased] - next" heading line: release notes get the
        # entries only, and an empty result means there is nothing to release.
        text = text.split("\n", 1)[1].strip("\n") if "\n" in text else ""
        if text:
            print(text)
        return 0
    print(text)
    return 0


def cmd_release(args):
    version = args.version.lstrip("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        print(f"'{version}' is not a version like 0.9.0", file=sys.stderr)
        return 2
    date = args.date or datetime.date.today().isoformat()
    fragments = read_fragments()
    changelog = CHANGELOG.read_text(encoding="utf-8")
    if f"## [{version}]" in changelog:
        print(f"CHANGELOG.md already has a section for {version}", file=sys.stderr)
        return 2
    before, body, after = split_unreleased(changelog)
    intro, existing = parse_sections(body)
    if not any(existing.values()) and not any(fragments.values()):
        print("nothing to release: no fragments and nothing under [Unreleased]", file=sys.stderr)
        return 2
    prev = previous_version(changelog)
    section = render(version, date, intro, existing, fragments)
    changelog = before + "\n" + section + "\n" + after
    changelog = changelog.replace(
        f"[Unreleased]: {REPO}/compare/v{prev}...HEAD\n",
        f"[Unreleased]: {REPO}/compare/v{version}...HEAD\n"
        f"[{version}]: {REPO}/compare/v{prev}...v{version}\n",
        1,
    )
    changelog = re.sub(r"\n{3,}", "\n\n", changelog)
    CHANGELOG.write_text(changelog, encoding="utf-8")
    for path in FRAGMENTS.glob("*.md"):
        if path.name != "README.md":
            path.unlink()
    print(f"cut {version} ({date}); review CHANGELOG.md, then commit with the version bump")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("check")
    prev = sub.add_parser("preview")
    prev.add_argument("--notes", action="store_true")
    rel = sub.add_parser("release")
    rel.add_argument("version")
    rel.add_argument("--date")
    args = parser.parse_args(argv)
    try:
        return {"check": cmd_check, "preview": cmd_preview, "release": cmd_release}[args.cmd](args)
    except FragmentError as err:
        print(err, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
