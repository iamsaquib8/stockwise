# MCP registry entry and Claude Desktop extension

`stockwise mcp` is the MCP server (see [docs/ai.md](../../docs/ai.md)). This
folder and the repository's [`server.json`](../../server.json) make it easy to
find and install.

## Claude Desktop extension (`.mcpb`)

A [`.mcpb`](https://github.com/modelcontextprotocol/mcpb) bundle is a zip of
[`manifest.json`](manifest.json) and the `stockwise` binary for one platform.
Claude Desktop installs it with a double-click (Settings → Extensions) and
asks for the default market.

```bash
# from a release archive's binary
packaging/mcpb/build.sh stockwise-aarch64-apple-darwin/stockwise aarch64-apple-darwin dist
packaging/mcpb/build.sh stockwise-x86_64-pc-windows-msvc/stockwise.exe x86_64-pc-windows-msvc dist
npx @anthropic-ai/mcpb validate packaging/mcpb/manifest.json
```

The script sets the version from `Cargo.toml`, limits `compatibility.platforms`
to the binary's platform, and prints the bundle's SHA-256. Attach the bundles
to the GitHub release (one per platform). The manifest's tool list must match
`stockwise mcp --list-tools`; a unit test checks it. An `icon.png` (512×512)
can be added to the bundle with an `"icon": "icon.png"` entry.

## Official MCP registry

[`server.json`](../../server.json) describes the server as
`io.github.iamsaquib8/stockwise` with three ways to run it: the crate
(`cargo install stockwise`), the PyPI package (`uvx stockwise mcp`) and the
GHCR image. The registry checks ownership:

- crates.io and PyPI: the README shows `mcp-name: io.github.iamsaquib8/stockwise`
  (visible text — crates.io strips HTML comments). The README of the PyPI
  package needs the same line.
- GHCR: the image's `io.modelcontextprotocol.server.name` label (in the
  [Dockerfile](../../Dockerfile)).

After a release is on crates.io, PyPI and GHCR, with its version in
`server.json`:

```bash
brew install mcp-publisher          # or the release binary from github.com/modelcontextprotocol/registry
mcp-publisher login github          # as the iamsaquib8 account
mcp-publisher publish               # reads server.json
```

Then open a pull request adding StockWise to
[awesome-mcp-servers](https://github.com/punkpeye/awesome-mcp-servers) (Finance
& Fintech).
