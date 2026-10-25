#!/usr/bin/env python3
"""Download and traffic numbers for StockWise, from public APIs.

StockWise itself sends no telemetry. This script reads the counters the
places we publish to already keep, so we can see which install paths people
use. The nightly `.github/workflows/stats.yml` runs `collect` and commits the
results to the `stats` branch; `summary` prints them.

    scripts/download-stats.py collect --out DIR   # add today's numbers to DIR
    scripts/download-stats.py summary              # read the stats branch on GitHub
    scripts/download-stats.py summary --dir DIR    # or a local copy

Files in DIR (the `stats` branch):

    totals.csv              date,metric,value: running totals, one set per day
    daily.csv               date,metric,value: per-day counts as the source reports them
    snapshots/DATE.json     everything collected that day, incl. referrers and paths

Environment: GITHUB_TOKEN (release counts, higher rate limit) and
STATS_TOKEN (optional; repository traffic needs a token with Administration:
read on the repositories, which the Actions GITHUB_TOKEN does not have).
Python 3.9+, standard library only.
"""

import argparse
import csv
import datetime as dt
import io
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

REPO = os.environ.get("STATS_REPO", "iamsaquib8/stockwise")
OWNER, NAME = REPO.split("/")
TAP_REPO = f"{OWNER}/homebrew-tap"
CRATE = "stockwise"
PYPI = "stockwise"
DOCKERHUB = os.environ.get("STATS_DOCKERHUB", f"{OWNER}/stockwise")
AUR = "stockwise-bin"
HOMEBREW_FORMULAE = ("stockwise", f"{OWNER}/tap/stockwise")
RAW = f"https://raw.githubusercontent.com/{REPO}/stats"
USER_AGENT = f"stockwise-download-stats (+https://github.com/{REPO})"


class NotFound(Exception):
    pass


def get(url, token=None, accept="application/json"):
    headers = {"User-Agent": USER_AGENT, "Accept": accept}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    try:
        with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as resp:
            body = resp.read().decode()
    except urllib.error.HTTPError as e:
        if e.code == 404:
            raise NotFound(url) from e
        raise
    return json.loads(body) if "json" in accept else body


def github(path, token):
    return get(f"https://api.github.com{path}", token, "application/vnd.github+json")


def collect_github(token):
    repo = github(f"/repos/{REPO}", token)
    totals = {
        "github.stars": repo["stargazers_count"],
        "github.forks": repo["forks_count"],
        "github.watchers": repo["subscribers_count"],
    }
    # Release assets keep their names across versions, so totals per asset
    # name are totals per platform/format. Checksums are counted apart: the
    # installer fetches one per install.
    releases, page = [], 1
    while True:
        batch = github(f"/repos/{REPO}/releases?per_page=100&page={page}", token)
        releases += batch
        if len(batch) < 100:
            break
        page += 1
    by_asset, by_release = {}, {}
    downloads = checksums = 0
    for release in releases:
        for asset in release["assets"]:
            count = asset["download_count"]
            if asset["name"].endswith(".sha256"):
                checksums += count
                continue
            downloads += count
            by_asset[asset["name"]] = by_asset.get(asset["name"], 0) + count
            by_release[release["tag_name"]] = by_release.get(release["tag_name"], 0) + count
    totals["github.release_downloads"] = downloads
    totals["github.checksum_downloads"] = checksums
    for name, count in by_asset.items():
        totals[f"github.asset.{name}"] = count
    return totals, {"releases": by_release, "assets": by_asset}


def collect_traffic(token, repo, prefix):
    """Views, clones (per day, last 14 days), referrers and popular paths."""
    daily = {}
    for kind in ("views", "clones"):
        data = github(f"/repos/{repo}/traffic/{kind}", token)
        for day in data[kind]:
            date = day["timestamp"][:10]
            daily[(date, f"{prefix}.{kind}")] = day["count"]
            daily[(date, f"{prefix}.unique_{kind}")] = day["uniques"]
    extra = {
        "referrers": github(f"/repos/{repo}/traffic/popular/referrers", token),
        "paths": github(f"/repos/{repo}/traffic/popular/paths", token),
    }
    return daily, extra


def collect_crates():
    crate = get(f"https://crates.io/api/v1/crates/{CRATE}")["crate"]
    totals = {"crates.downloads": crate["downloads"], "crates.recent_downloads_90d": crate["recent_downloads"]}
    data = get(f"https://crates.io/api/v1/crates/{CRATE}/downloads")
    daily = {}
    for row in data["version_downloads"] + data["meta"]["extra_downloads"]:
        key = (row["date"], "crates.downloads")
        daily[key] = daily.get(key, 0) + row["downloads"]
    return totals, daily


def collect_pypi():
    recent = get(f"https://pypistats.org/api/packages/{PYPI}/recent")["data"]
    totals = {
        "pypi.last_day": recent["last_day"],
        "pypi.last_week": recent["last_week"],
        "pypi.last_month": recent["last_month"],
    }
    overall = get(f"https://pypistats.org/api/packages/{PYPI}/overall?mirrors=false")["data"]
    daily = {(row["date"], "pypi.downloads"): row["downloads"] for row in overall}
    return totals, daily


def collect_homebrew():
    """Install counts from Homebrew's public analytics (core and tap formulae)."""
    totals, extra = {}, {}
    for period in ("30d", "90d", "365d"):
        items = get(f"https://formulae.brew.sh/api/analytics/install/{period}.json")["items"]
        for item in items:
            if item["formula"] in HOMEBREW_FORMULAE:
                where = "core" if "/" not in item["formula"] else "tap"
                totals[f"homebrew.{where}.installs_{period}"] = int(item["count"].replace(",", ""))
                extra[f"{where}_{period}_rank"] = item["number"]
    return totals, extra


def collect_ghcr():
    # The package API has no download counts; the package page shows them.
    page = get(f"https://github.com/{REPO}/pkgs/container/{NAME}", accept="text/html")
    m = re.search(r'Total downloads</span>\s*<h3 title="(\d+)"', page)
    return {"ghcr.pulls": int(m.group(1))} if m else {}


def collect_dockerhub():
    repo = get(f"https://hub.docker.com/v2/repositories/{DOCKERHUB}/")
    return {"dockerhub.pulls": repo["pull_count"], "dockerhub.stars": repo["star_count"]}


def collect_aur():
    results = get(f"https://aur.archlinux.org/rpc/v5/info?arg[]={AUR}")["results"]
    if not results:
        raise NotFound(AUR)
    return {"aur.votes": results[0]["NumVotes"], "aur.popularity": results[0]["Popularity"]}


def merge_csv(path, rows, replace_dates=()):
    """Upsert (date, metric) -> value rows into a date,metric,value CSV."""
    table = {}
    if path.exists():
        with path.open(newline="") as f:
            for row in csv.DictReader(f):
                if row["date"] not in replace_dates:
                    table[(row["date"], row["metric"])] = row["value"]
    table.update({key: str(value) for key, value in rows.items()})
    with path.open("w", newline="") as f:
        writer = csv.writer(f, lineterminator="\n")
        writer.writerow(["date", "metric", "value"])
        for (date, metric), value in sorted(table.items()):
            writer.writerow([date, metric, value])


def collect(out):
    today = dt.datetime.now(dt.timezone.utc).date().isoformat()
    token = os.environ.get("GITHUB_TOKEN") or None
    traffic_token = os.environ.get("STATS_TOKEN") or None
    totals, daily = {}, {}
    snapshot = {"date": today, "repository": REPO, "sources": {}}

    def source(name, fn):
        try:
            result = fn()
        except NotFound:
            print(f"  {name}: not published yet")
            snapshot["sources"][name] = "not found"
            return None
        except (urllib.error.URLError, KeyError, ValueError, TimeoutError) as e:
            print(f"  {name}: failed ({e})", file=sys.stderr)
            snapshot["sources"][name] = f"failed: {e}"
            return None
        snapshot["sources"][name] = "ok"
        print(f"  {name}: ok")
        return result

    if result := source("github", lambda: collect_github(token)):
        totals.update(result[0])
        snapshot["github"] = result[1]
    if result := source("crates.io", collect_crates):
        totals.update(result[0])
        daily.update(result[1])
    if result := source("pypi", collect_pypi):
        totals.update(result[0])
        daily.update(result[1])
    if result := source("homebrew", collect_homebrew):
        totals.update(result[0])
        snapshot["homebrew"] = result[1]
    for name, fn in (("ghcr", collect_ghcr), ("dockerhub", collect_dockerhub), ("aur", collect_aur)):
        if result := source(name, fn):
            totals.update(result)
    if traffic_token:
        for repo, prefix in ((REPO, "traffic"), (TAP_REPO, "tap_traffic")):
            if result := source(f"traffic {repo}", lambda r=repo, p=prefix: collect_traffic(traffic_token, r, p)):
                daily.update(result[0])
                snapshot[prefix] = result[1]
    else:
        print("  traffic: skipped (set STATS_TOKEN to collect views, clones and referrers)")
        snapshot["sources"]["traffic"] = "skipped: no STATS_TOKEN"

    snapshot["totals"] = totals
    out.mkdir(parents=True, exist_ok=True)
    (out / "snapshots").mkdir(exist_ok=True)
    (out / "snapshots" / f"{today}.json").write_text(json.dumps(snapshot, indent=2, sort_keys=True) + "\n")
    merge_csv(out / "totals.csv", {(today, k): v for k, v in totals.items()}, replace_dates={today})
    merge_csv(out / "daily.csv", daily)
    print(f"Wrote {len(totals)} totals and {len(daily)} daily counts to {out}")
    return 0


def read_csv(location, name):
    if location:
        path = Path(location) / name
        if not path.exists():
            return []
        text = path.read_text()
    else:
        try:
            text = get(f"{RAW}/{name}", accept="text/csv")
        except NotFound:
            return []
    return list(csv.DictReader(io.StringIO(text)))


def number(value):
    n = float(value)
    return int(n) if n.is_integer() else round(n, 2)


def summary(location):
    totals = read_csv(location, "totals.csv")
    if not totals:
        print("No stats yet. Run the 'Download stats' workflow, or `collect --out DIR` locally.")
        return 1
    by_date = {}
    for row in totals:
        by_date.setdefault(row["date"], {})[row["metric"]] = number(row["value"])
    dates = sorted(by_date)
    latest = dates[-1]

    def back(days):
        cutoff = (dt.date.fromisoformat(latest) - dt.timedelta(days=days)).isoformat()
        older = [d for d in dates if d <= cutoff]
        return by_date[older[-1]] if older else {}

    windows = [("1d", back(1)), ("7d", back(7)), ("30d", back(30))]
    print(f"StockWise download stats, {latest} (deltas against the nearest earlier day)\n")
    print(f"{'metric':<56}{'total':>10}" + "".join(f"{label:>8}" for label, _ in windows))
    for metric in sorted(by_date[latest]):
        value = by_date[latest][metric]
        cells = []
        for _, old in windows:
            cells.append(f"{value - old[metric]:+8}" if metric in old else f"{'':>8}")
        print(f"{metric:<56}{value:>10}" + "".join(cells))

    daily = read_csv(location, "daily.csv")
    if daily:
        sums = {}
        last = max(row["date"] for row in daily)
        start = (dt.date.fromisoformat(last) - dt.timedelta(days=29)).isoformat()
        for row in daily:
            if row["date"] >= start:
                sums[row["metric"]] = sums.get(row["metric"], 0) + number(row["value"])
        print(f"\nLast 30 days of per-day counts (to {last}):\n")
        for metric, value in sorted(sums.items()):
            print(f"{metric:<56}{value:>10}")

    try:
        snap = json.loads(Path(location, "snapshots", f"{latest}.json").read_text()) if location else \
            get(f"{RAW}/snapshots/{latest}.json")
    except (NotFound, OSError):
        snap = {}
    referrers = snap.get("traffic", {}).get("referrers", [])
    if referrers:
        print("\nTop referrers (last 14 days):\n")
        for ref in referrers:
            print(f"{ref['referrer']:<44}{ref['count']:>8} views{ref['uniques']:>6} unique")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("collect", help="fetch today's numbers and add them to DIR")
    p.add_argument("--out", type=Path, required=True, metavar="DIR")
    p = sub.add_parser("summary", help="print the latest numbers and how they changed")
    p.add_argument("--dir", metavar="DIR", help="a local copy of the stats branch (default: read it from GitHub)")
    args = parser.parse_args()
    return collect(args.out) if args.command == "collect" else summary(args.dir)


if __name__ == "__main__":
    sys.exit(main())
