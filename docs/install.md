# Install

StockWise is one binary with no runtime dependencies. Pick whichever way
you already use to install tools; they all give you the same `stockwise`.

## Quick install

macOS, Linux and Termux on Android:

```bash
curl -fsSL https://raw.githubusercontent.com/iamsaquib8/stockwise/master/install.sh | sh
```

The installer picks the right build for your system, checks its SHA-256
against the release, and installs to `~/.local/bin` (`$PREFIX/bin` on
Termux). `STOCKWISE_INSTALL=/usr/local/bin` changes the directory and
`STOCKWISE_VERSION=v0.5.0` pins a version.

## Every option

| Platform | Command | Man pages and completions | Updates |
|----------|---------|:---:|---------|
| macOS, Linux | `brew install iamsaquib8/tap/stockwise` ¹ | ✓ | `brew upgrade` |
| Any (Python) | `pipx install stockwise` or `uv tool install stockwise` ² | — | `pipx upgrade stockwise` / `uv tool upgrade stockwise` |
| Any (Python), no install | `uvx stockwise quote RELIANCE` ² | — | always the latest |
| Debian 12+, Ubuntu 22.04+ | `curl -fsSLO https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise_amd64.deb && sudo apt install ./stockwise_amd64.deb` ² | ✓ | download the new `.deb` |
| Fedora, RHEL 9+ | `sudo dnf install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise-x86_64.rpm` ² | ✓ | run the same command again |
| Arch Linux | `yay -S stockwise-bin` ¹ | ✓ | `yay -Syu` |
| Nix | `nix profile install github:iamsaquib8/stockwise` | ✓ | `nix profile upgrade stockwise` |
| NixOS / nixpkgs | `nix profile install nixpkgs#stockwise` ³ | ✓ | with your channel |
| Android (Termux) | the quick install above ² | — | run it again |
| Android (Termux) | `pkg install stockwise` ³ | ✓ | `pkg upgrade` |
| Windows | `scoop install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise.json` | — | `scoop update stockwise` |
| Windows | `winget install iamsaquib8.StockWise` ¹ | — | `winget upgrade` |
| Docker | `docker run --rm -it -v stockwise-data:/data ghcr.io/iamsaquib8/stockwise quote TCS` | — | `docker pull` |
| Docker Hub | `docker run --rm -it -v stockwise-data:/data iamsaquib8/stockwise quote TCS` ¹ | — | `docker pull` |
| Rust | `cargo binstall stockwise` (prebuilt) or `cargo install stockwise` | — | run it again |
| Any | archives from the [latest release](https://github.com/iamsaquib8/stockwise/releases/latest) | man pages included | download again |

¹ Needs a one-time publishing step by the maintainer (tap repository,
AUR/winget submission, Docker Hub credentials); see
[packaging](https://github.com/iamsaquib8/stockwise/blob/master/packaging/README.md).
² 0.6.0 and later; PyPI also needs its one-time trusted-publisher setup.
³ Prepared and waiting to be submitted to nixpkgs and termux-packages.

On ARM Linux, use `stockwise_arm64.deb` or `stockwise-aarch64.rpm`. The Linux
binaries (archives, quick install, `.deb`/`.rpm`) need glibc 2.34 or newer:
Ubuntu 22.04, Debian 12, RHEL 9 and later. The PyPI wheels are built for
manylinux (glibc 2.17+) and musllinux, so `pipx` also works on older and
Alpine systems.

Where completions are not installed for you:

```bash
stockwise completions zsh > ~/.zfunc/_stockwise           # bash, zsh, fish, powershell, elvish
stockwise man --dir ~/.local/share/man/man1               # then: man stockwise
```

## Check it

```bash
stockwise --version
stockwise doctor        # data directory, network, optional Redis/Ollama/keyring
```

## Privacy

StockWise sends no telemetry. The project counts downloads only from the
public numbers each registry already shows (crates.io, GitHub releases,
PyPI, Homebrew, Docker); see
[download stats](https://github.com/iamsaquib8/stockwise/blob/master/packaging/README.md#download-stats).
