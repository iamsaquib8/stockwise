#!/usr/bin/env python3
"""Build the documentation site into target/site with mdBook.

Copies docs/*.md, the site's own pages (SUMMARY.md, index.md) and the
top-level CHANGELOG, ROADMAP, CONTRIBUTING and DISCLAIMER into
target/site-src, rewrites links that point outside docs/ (images are copied
next to the pages, project pages become site pages, source files link to
GitHub), then runs `mdbook build site`.

After a build it also writes, for AI assistants (https://llmstxt.org):
- `llms.txt` — what StockWise is, the docs pages and the cheatsheet's commands
- `llms-full.txt` — every page in one Markdown file
- `<page>.md` next to each `<page>.html`, the Markdown the links point to

Usage: python3 site/build.py [--serve]   (needs mdBook on PATH)
"""

import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "target" / "site-src"
THEME = ROOT / "target" / "site-theme"
OUT = ROOT / "target" / "site"
REPO = "https://github.com/iamsaquib8/stockwise"
SITE_URL = "https://iamsaquib8.github.io/stockwise/"

# Top-level project files published as site pages.
PROJECT_PAGES = {
    "CHANGELOG.md": "changelog.md",
    "ROADMAP.md": "roadmap.md",
    "CONTRIBUTING.md": "contributing.md",
    "DISCLAIMER.md": "disclaimer.md",
}

LINK = re.compile(r"(!?\[[^\]]*\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")


def site_target(path_from_root: str) -> str:
    """Where a link to `path_from_root` (relative to the repo root) goes on the site."""
    path, _, frag = path_from_root.partition("#")
    frag = f"#{frag}" if frag else ""
    if path in PROJECT_PAGES:
        return PROJECT_PAGES[path] + frag
    if path.startswith("docs/"):
        return path[len("docs/"):] + frag
    if path.startswith("assets/"):
        return path + frag
    kind = "tree" if path.endswith("/") or "." not in Path(path).name else "blob"
    return f"{REPO}/{kind}/master/{path}{frag}"


def rewrite(text: str, base: str) -> str:
    """Rewrite relative links in a page that lives at `base` in the repo
    ("docs" for docs pages, "" for top-level files)."""

    def fix(m: re.Match) -> str:
        url = m.group(2)
        if re.match(r"^[a-z]+:|^#|^/", url):
            return m.group(0)
        if base == "docs":
            if not url.startswith("../"):
                return m.group(0)  # sibling page, already correct
            url = url[len("../"):]
        return m.group(1) + site_target(url) + m.group(3)

    return LINK.sub(fix, text)


# Descriptions for pages whose first sentence doesn't summarize them.
DESCRIPTIONS = {
    "bot-design.md": "How the intraday and long-term bots and the daemon work, and what is planned.",
    "contributing.md": "How to build StockWise, run the checks and send a change.",
    "architecture.md": "Map of the source tree, data flow, caching, storage and dependencies.",
}

SUMMARY_LINK = re.compile(r"^(\s*)(?:- )?\[([^\]]+)\]\(([^)]+\.md)\)")


def summary_pages() -> list[tuple[str, str, str]]:
    """(section, title, file) for every page in SUMMARY.md, in book order."""
    pages, section = [], ""
    for line in (ROOT / "site" / "SUMMARY.md").read_text().splitlines():
        if line.startswith("# ") and not line.startswith("# Summary"):
            section = line[2:].strip()
            continue
        m = SUMMARY_LINK.match(line)
        if m:
            pages.append((section, m.group(2), m.group(3)))
    return pages


def plain(md: str) -> str:
    """Markdown inline text without links, emphasis or HTML."""
    md = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", md)
    md = re.sub(r"</?(?:p|img|br|a|div|span|sup|sub|kbd|details|summary)\b[^>]*>", "", md)
    return md.replace("**", "").strip()


def summary_of(page: str) -> str:
    """The first sentence of a page's first paragraph."""
    para: list[str] = []
    in_code = False
    for line in page.splitlines():
        s = line.strip()
        if s.startswith("```"):
            in_code = not in_code
            continue
        if in_code:
            continue
        if not s:
            if para:
                break
            continue
        # Headings, tables, HTML, quotes, list items and their indented
        # continuation lines are not the opening paragraph.
        if line[0].isspace() or s.startswith(("#", "|", "<", ">", "- ", "* ", "!", "1.")):
            if para:
                break
            continue
        para.append(s)
    text = plain(" ".join(para))
    m = re.match(r"(.+?[.!?])(\s|$)", text)
    return (m.group(1) if m else text)[:240]


def site_links(md: str) -> str:
    """Relative page links as absolute site URLs, for text read off-site."""

    def fix(m: re.Match) -> str:
        url = m.group(2)
        if re.match(r"^[a-z]+:|^#|^/", url):
            return m.group(0)
        path, _, frag = url.partition("#")
        if path.endswith(".md"):
            path = path[:-3] + ".html"
        return m.group(1) + SITE_URL + path + (f"#{frag}" if frag else "") + m.group(3)

    return LINK.sub(fix, md)


def cheatsheet_rows() -> list[tuple[str, list[tuple[str, str]]]]:
    """(heading, [(task, command)]) from the cheatsheet's tables."""
    out: list[tuple[str, list[tuple[str, str]]]] = []
    for line in (ROOT / "docs" / "cheatsheet.md").read_text().splitlines():
        if line.startswith("## "):
            out.append((line[3:].strip(), []))
        elif line.startswith("| ") and out and "`" in line:
            cells = [c.strip() for c in re.split(r"(?<!\\)\|", line.strip().strip("|"))]
            if len(cells) >= 2:
                out[-1][1].append((plain(cells[0]), cells[1].replace("\\|", "|")))
    return [(h, rows) for h, rows in out if rows]


def write_llms_txt() -> None:
    """llms.txt, llms-full.txt and a .md copy of every page, in the built site."""
    pages = summary_pages()
    for _, _, name in pages:
        shutil.copy(SRC / name, OUT / name)

    lines = [
        "# StockWise",
        "",
        "> Stock market toolkit for the terminal, built for Indian (NSE/BSE) and US stocks: "
        "quotes, charts, technicals, screeners, mutual funds (AMFI), F&O option chains, "
        "commodities, G-secs, crypto and FX, backtests, paper trading, portfolio tracking "
        "with XIRR and capital-gains tax, goal planning. One binary, no API keys, no account. "
        "Also an MCP server (`stockwise mcp`) that gives AI assistants the same data as "
        "read-only tools.",
        "",
        "Key facts for agents:",
        "",
        "- Install: `cargo binstall stockwise`, `pipx install stockwise` / `uvx stockwise`, "
        "Homebrew, Scoop, winget, AUR, Docker (`ghcr.io/iamsaquib8/stockwise`), or the "
        "one-line installer. Check with `stockwise doctor`.",
        "- Bare tickers are Indian (`TCS` means `TCS.NS`); add `-m us`, `-m crypto`, `-m fx`, "
        "`-m commodity` or an exchange (`-m uk`, `de`, `jp`, `hk`, `sg`, `ca`, `au`).",
        "- Add `--json` for machine-readable output (most commands; tables also take `--csv`). "
        "stdout then holds only JSON; errors go to stderr with exit code 1.",
        "- Prefer the MCP server when it is connected: `stockwise mcp` (stdio) or "
        "`stockwise mcp --http 127.0.0.1:8788`. Its tools are read-only.",
        "- Live orders (`stockwise trade`) move real money: never run them for a user without "
        "their explicit request, and they always ask for a typed confirmation.",
        "- Output is educational information, not financial advice.",
        "",
        "## Docs",
        "",
    ]
    for section, title, name in pages:
        if section == "Project":
            continue
        desc = DESCRIPTIONS.get(name) or summary_of((SRC / name).read_text())
        lines.append(f"- [{title}]({SITE_URL}{name}): {desc}")
    lines += [
        "",
        "## AI assistants",
        "",
        f"- [MCP server and `stockwise ask`]({SITE_URL}ai.md): tools, resources, prompts, "
        "client setup for Claude Desktop, Claude Code and Cursor",
        f"- [Agent Skill]({REPO}/blob/master/skills/stockwise/SKILL.md): how an agent should "
        "use the CLI and the MCP server",
        f"- [MCP registry entry]({REPO}/blob/master/server.json): `io.github.iamsaquib8/stockwise`",
        "",
        "## Commands",
        "",
    ]
    for heading, rows in cheatsheet_rows():
        lines.append(f"### {heading}")
        lines.append("")
        lines += [f"- {task}: {cmd}" for task, cmd in rows]
        lines.append("")
    lines += ["## Optional", ""]
    for section, title, name in pages:
        if section == "Project":
            desc = DESCRIPTIONS.get(name) or summary_of((SRC / name).read_text())
            lines.append(f"- [{title}]({SITE_URL}{name}): {desc}")
    (OUT / "llms.txt").write_text("\n".join(lines).rstrip() + "\n")

    full = [
        "# StockWise — full documentation",
        "",
        f"Every page of {SITE_URL} in one file, in the order of the site's table of contents.",
    ]
    for _, title, name in pages:
        full += ["", "---", "", f"<!-- {SITE_URL}{name} -->", ""]
        full.append(site_links((SRC / name).read_text()).strip())
    (OUT / "llms-full.txt").write_text("\n".join(full) + "\n")


def main() -> int:
    if SRC.exists():
        shutil.rmtree(SRC)
    (SRC / "assets").mkdir(parents=True)

    for page in sorted((ROOT / "docs").glob("*.md")):
        (SRC / page.name).write_text(rewrite(page.read_text(), "docs"))
    for name in ("SUMMARY.md", "index.md"):
        shutil.copy(ROOT / "site" / name, SRC / name)
    for source, dest in PROJECT_PAGES.items():
        (SRC / dest).write_text(rewrite((ROOT / source).read_text(), ""))
    for image in (ROOT / "assets").glob("*.svg"):
        shutil.copy(image, SRC / "assets" / image.name)

    THEME.mkdir(parents=True, exist_ok=True)
    shutil.copy(ROOT / "assets" / "logo.svg", THEME / "favicon.svg")

    serve = "--serve" in sys.argv
    cmd = ["mdbook", "serve" if serve else "build", str(ROOT / "site")]
    code = subprocess.call(cmd)
    if code == 0 and not serve:
        write_llms_txt()
    return code


if __name__ == "__main__":
    sys.exit(main())
