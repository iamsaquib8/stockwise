# Install

StockWise is one binary with no runtime dependencies. Pick whichever way
you already use to install tools; they all give you the same `stockwise`.

## Quick install

macOS, Linux and Termux on Android:

```bash
curl -fsSL https://raw.githubusercontent.com/iamsaquib8/stockwise/main/install.sh | sh
```

The installer picks the right build for your system, checks its SHA-256
against the release, and installs to `~/.local/bin` (`$PREFIX/bin` on
Termux). `STOCKWISE_INSTALL=/usr/local/bin` changes the directory and
`STOCKWISE_VERSION=v0.8.0` pins a version.

## Every option

These work today with the [latest release](https://github.com/iamsaquib8/stockwise/releases/latest):

| Platform | Command | Man pages and completions | Updates |
|----------|---------|:---:|---------|
| macOS, Linux, Termux | the quick install above | — | run it again |
| Debian 12+, Ubuntu 22.04+ | `curl -fsSLO https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise_amd64.deb && sudo apt install ./stockwise_amd64.deb` | ✓ | download the new `.deb` |
| Fedora, RHEL 9+ | `sudo dnf install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise-x86_64.rpm` | ✓ | run the same command again |
| Nix | `nix profile install github:iamsaquib8/stockwise` | ✓ | `nix profile upgrade stockwise` |
| Windows | `scoop install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise.json` | — | `scoop update stockwise` |
| Docker | `docker run --rm -it -v stockwise-data:/data ghcr.io/iamsaquib8/stockwise quote TCS` | — | `docker pull` |
| Rust | `cargo binstall stockwise` (prebuilt) or `cargo install --locked stockwise` | — | run it again |
| Any | archives from the [latest release](https://github.com/iamsaquib8/stockwise/releases/latest) | man pages included | download again |

### Coming soon

Prepared, each waiting on a one-time publishing step by the maintainer (see
[packaging](https://github.com/iamsaquib8/stockwise/blob/main/packaging/README.md)):

| Platform | Command |
|----------|---------|
| Any (Python) | `pipx install stockwise`, `uv tool install stockwise` or `uvx stockwise quote RELIANCE` |
| macOS, Linux | `brew install iamsaquib8/tap/stockwise` |
| Windows | `winget install iamsaquib8.StockWise` |
| Arch Linux | `yay -S stockwise-bin` |
| Docker Hub | `docker run --rm -it -v stockwise-data:/data iamsaquib8/stockwise quote TCS` |
| NixOS / nixpkgs | `nix profile install nixpkgs#stockwise` |
| Android (Termux) | `pkg install stockwise` |

On ARM Linux, use `stockwise_arm64.deb` or `stockwise-aarch64.rpm`. The Linux
binaries (archives, quick install, `.deb`/`.rpm`) need glibc 2.34 or newer:
Ubuntu 22.04, Debian 12, RHEL 9 and later. Once PyPI is live, its wheels
(manylinux, glibc 2.17+, and musllinux) will also cover older and Alpine
systems.

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

If a command fails, [Troubleshooting](troubleshooting.md) lists the common
messages and what to do about each.

## Next steps

- Run the [five-minute quickstart](quickstart.md).
- Pick a goal on [What do you want to do?](guides.md).
- Something wrong? See [Troubleshooting](troubleshooting.md).

## Privacy

StockWise sends no telemetry. The project counts downloads only from the
public numbers each registry already shows (crates.io, GitHub releases,
PyPI, Homebrew, Docker); see
[download stats](https://github.com/iamsaquib8/stockwise/blob/main/packaging/README.md#download-stats).
