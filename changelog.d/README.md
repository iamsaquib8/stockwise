# Changelog fragments

Each user-visible change adds **one small file** here instead of editing
`CHANGELOG.md`. Two pull requests then never touch the same lines, so the
changelog no longer conflicts. The release script collects the fragments.

## Naming

`<issue-number-or-slug>.<type>.md`, where `type` is one of:

| type | section |
|---|---|
| `added` | Added |
| `changed` | Changed |
| `deprecated` | Deprecated |
| `removed` | Removed |
| `fixed` | Fixed |
| `performance` | Performance |
| `security` | Security |

Examples: `123.fixed.md`, `xray-etfs.changed.md`.

## Content

One bullet, written like any entry in `CHANGELOG.md` (see "Changelog entries" in
[CONTRIBUTING.md](../CONTRIBUTING.md)): one to three lines, plain language,
starts with the command name in backticks, links the issue, credits outside
contributors. The file starts with `- `; wrapped lines are indented by two
spaces.

```markdown
- `tax in` counts gold ETFs as non-equity, so they are no longer taxed as listed
  shares ([#87](https://github.com/iamsaquib8/stockwise/issues/87)).
```

## Commands

```bash
python3 scripts/changelog.py check              # validate the fragments (CI runs this)
python3 scripts/changelog.py preview            # print the next release's entries
python3 scripts/changelog.py preview --notes    # the entries only, as the release workflow uses them
python3 scripts/changelog.py release 0.9.0      # maintainers: cut a release
```

`release` moves the fragments and anything left under `[Unreleased]` into a new
`## [0.9.0] - <today>` section, updates the compare links at the bottom, and
deletes the fragment files. Commit the result with the version bump.
