# Packaging

How StockWise gets from a release tag to package managers. Version numbers and
checksums are never edited by hand: `render.sh` fills them in from a release's
`.sha256` files.

| Channel | Source | How it is published |
|---------|--------|---------------------|
| Release archives | `.github/workflows/release.yml` | Automatic on every `v*` tag: binary, README, LICENSE, CHANGELOG, DISCLAIMER and `man/*.1`; also `aarch64-linux-android` for Termux |
| `.deb` / `.rpm` | `linux/build-packages.sh`, `[package.metadata.deb]` and `[package.metadata.generate-rpm]` in `../Cargo.toml` | Automatic on every `v*` tag: `stockwise_{amd64,arm64}.deb` and `stockwise-{x86_64,aarch64}.rpm` with man pages and completions |
| PyPI | `../pyproject.toml` (maturin `bindings = "bin"`), `pypi/README.md` | `pypi-wheels` + `pypi-sdist` build on every tag and manual run; `pypi-publish` uploads only when the `PYPI_PUBLISH` variable is `true` (trusted publishing) |
| Homebrew | `homebrew/stockwise.rb` | `homebrew` job pushes `Formula/stockwise.rb` to `iamsaquib8/homebrew-tap`, only when the `HOMEBREW_TAP_TOKEN` secret exists |
| Scoop | `scoop/stockwise.json` | `packaging` job attaches the rendered manifest to the release (`scoop install <release URL>`); `checkver`/`autoupdate` also suit a bucket |
| winget | `winget/*.yaml` | Manual pull request to `microsoft/winget-pkgs` |
| AUR | `aur/stockwise-bin/` | Manual push to `aur.archlinux.org/stockwise-bin.git` |
| Docker | `../Dockerfile` | `docker` + `docker-manifest` jobs push `ghcr.io/iamsaquib8/stockwise:<version>`, `:<major>.<minor>` and `:latest` (linux/amd64, linux/arm64) |
| Docker Hub | `docker/README.md` | `docker-manifest` copies the same tags to `docker.io/iamsaquib8/stockwise` and updates its description, only when `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` exist |
| Nix | `../flake.nix` | Builds from source; nothing to publish |
| nixpkgs | `nixpkgs/package.nix` | Manual pull request to `NixOS/nixpkgs` ([nixpkgs/README.md](nixpkgs/README.md)) |
| Termux | `termux/build.sh` | Manual pull request to `termux/termux-packages` ([termux/README.md](termux/README.md)); until then `install.sh` installs the Android build |
| Download stats | `../scripts/download-stats.py`, `../.github/workflows/stats.yml` | Nightly; commits counts to the `stats` branch |

## Man pages

`stockwise man --dir man` writes `stockwise.1` and one `stockwise-<command>.1`
per command, rendered by `clap_mangen` from the CLI definition in `src/main.rs`.
The release workflow runs it once (`manpages` job) and puts the pages in every
archive under `man/`. Homebrew, the AUR package and the Nix flake install them.
There is no hand-written copy to keep in sync.

## Templates

Placeholders: `@VERSION@` and `@SHA256_<TARGET>@` (the Rust target in upper case
with `_` for `-`, e.g. `@SHA256_AARCH64_APPLE_DARWIN@`). Lines starting with
`#:` are template notes and are dropped from the output.

```bash
gh release download v0.3.0 -R iamsaquib8/stockwise --pattern '*.sha256' --dir sums
packaging/render.sh 0.3.0 sums dist
```

Every tagged release also uploads the rendered set as the `packaging-manifests`
workflow artifact, so the manual steps below can start from it.

## One-time setup (maintainer)

1. **Homebrew tap.** Create the public repository `iamsaquib8/homebrew-tap`.
   Create a fine-grained personal access token with *Contents: read and write*
   on that repository only, and save it as the Actions secret
   `HOMEBREW_TAP_TOKEN` in `iamsaquib8/stockwise`. The next tag pushes the
   formula. For a release that is already out, render it as above and commit
   `dist/homebrew/stockwise.rb` to `Formula/stockwise.rb` in the tap.
2. **Container visibility.** After the first tagged release, open the
   `stockwise` package under your GitHub profile → Packages → Package settings,
   and set its visibility to Public.
3. **winget.** Fork `microsoft/winget-pkgs`, copy
   `dist/winget/manifests/i/iamsaquib8/StockWise/<version>/` to the same path,
   check it with `winget validate --manifest <that dir>` on Windows, and open a
   pull request. Repeat per release (or automate it later with a winget
   releaser action and a token).
4. **AUR.** With an AUR account and SSH key:
   `git clone ssh://aur@aur.archlinux.org/stockwise-bin.git`, copy in
   `dist/aur/stockwise-bin/PKGBUILD` and `.SRCINFO`, test with `makepkg -si`
   (and `namcap`), then commit and push. Repeat per release.
5. **Scoop bucket (optional).** The release URL already works. For
   `scoop install stockwise`, create `iamsaquib8/scoop-bucket` with
   `bucket/stockwise.json`; Scoop's `checkver`/`autoupdate` keep it current.
6. **PyPI** (`pipx install stockwise`, `uvx stockwise`). The name `stockwise`
   was free on PyPI on 2026-10-26; nothing reserves it until the first upload.
   1. Sign in to <https://pypi.org> (enable 2FA) → *Your projects* →
      *Publishing* → *Add a new pending publisher* → GitHub, with
      PyPI project name `stockwise`, owner `iamsaquib8`, repository
      `stockwise`, workflow `release.yml`, environment `pypi`.
   2. In `iamsaquib8/stockwise` → Settings → Environments, create `pypi`
      (optionally restrict it to `v*` tags and require your approval).
   3. Settings → Secrets and variables → Actions → *Variables*: add
      `PYPI_PUBLISH` = `true`.
   4. The next `v*` tag publishes the wheels and sdist (tags before this
      change have no PyPI jobs, so 0.5.0 itself is not on PyPI).
7. **Docker Hub mirror.** Create the public repository
   `iamsaquib8/stockwise` on <https://hub.docker.com>. Create a personal
   access token (Account settings → Personal access tokens) with *Read, Write,
   Delete* (the description update needs Delete). Add the Actions secrets
   `DOCKERHUB_USERNAME` (your Docker Hub user) and `DOCKERHUB_TOKEN`. Under a
   different namespace, also set the variable `DOCKERHUB_REPOSITORY` (e.g.
   `myorg/stockwise`). The next tag copies the image and the description.
8. **nixpkgs** and **Termux.** See [nixpkgs/README.md](nixpkgs/README.md) and
   [termux/README.md](termux/README.md); both are one pull request, after
   which bots keep the versions current.
9. **Download stats.** Nothing to set up for the public counters; the first
   nightly run (or *Run workflow* on *Download stats*) creates the `stats`
   branch. For repository traffic (views, clones, referrers), create a
   fine-grained token with *Administration: Read-only* on `stockwise` and
   `homebrew-tap` and save it as the secret `STATS_TOKEN`. GitHub keeps
   traffic for 14 days only, so the nightly archive is the history. GitHub
   disables scheduled workflows after 60 days without repository activity;
   re-enable it from the Actions tab if that happens.

## Download stats

StockWise has no telemetry. `scripts/download-stats.py` reads public
counters instead: crates.io (total and per day), GitHub release assets (per
asset name, so per platform and format, with checksum downloads counted
apart — `install.sh` fetches one per install), GHCR and Docker Hub pulls,
PyPI (pypistats.org, without mirrors), Homebrew install analytics for 30, 90
and 365 days (core and tap formula), AUR votes, stars and forks, and with
`STATS_TOKEN` the views, clones, referrers and popular pages of this
repository and the tap (tap clones approximate `brew` installs and updates).

```bash
scripts/download-stats.py summary                  # latest numbers from the stats branch
git fetch origin stats && git worktree add /tmp/stats origin/stats
scripts/download-stats.py summary --dir /tmp/stats # or from a local copy
```

## Checking changes

`.github/workflows/packaging.yml` runs on pull requests that touch these files:
it renders the templates, compares the AUR `.SRCINFO` with `makepkg
--printsrcinfo`, builds and smoke-tests the Docker image, runs
`nix flake check` and builds `nixpkgs/package.nix`, builds and installs the
`.deb` (Ubuntu) and `.rpm` (Fedora), builds a manylinux wheel and the sdist and
runs the wheel through `pipx`, and builds for `aarch64-linux-android`. Locally:

```bash
docker build -t stockwise . && docker run --rm stockwise --version
nix flake check && nix run . -- --version
uvx maturin build --release && uvx --from target/wheels/stockwise-*.whl stockwise --version
cargo build --release --target x86_64-unknown-linux-gnu &&
  packaging/linux/build-packages.sh x86_64-unknown-linux-gnu dist   # on Linux, with cargo-deb and cargo-generate-rpm
```
