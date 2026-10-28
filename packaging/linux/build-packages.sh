#!/bin/sh
# Build the .deb and .rpm for one Linux target from an already-built binary.
#
#   cargo build --release --locked --target x86_64-unknown-linux-gnu
#   packaging/linux/build-packages.sh x86_64-unknown-linux-gnu dist
#
# Needs cargo-deb and cargo-generate-rpm on PATH, and a binary that runs on
# this machine: the man pages and shell completions come from the binary
# itself, so they always match the CLI. Package metadata lives in Cargo.toml
# ([package.metadata.deb] and [package.metadata.generate-rpm]).
#
# Writes version-less names, so the latest release has stable download URLs:
#
#   OUT_DIR/stockwise_amd64.deb   OUT_DIR/stockwise-x86_64.rpm    (+ .sha256)
#   OUT_DIR/stockwise_arm64.deb   OUT_DIR/stockwise-aarch64.rpm
set -eu

die() { printf 'build-packages.sh: %s\n' "$*" >&2; exit 1; }

[ $# -eq 2 ] || die "usage: build-packages.sh TARGET OUT_DIR"
target=$1
out=$2

case "$target" in
  x86_64-unknown-linux-gnu) deb_arch=amd64 rpm_arch=x86_64 ;;
  aarch64-unknown-linux-gnu) deb_arch=arm64 rpm_arch=aarch64 ;;
  *) die "unsupported target '$target'" ;;
esac

cd "$(dirname "$0")/../.."
bin="target/$target/release/stockwise"
[ -x "$bin" ] || die "no binary at $bin; run cargo build --release --target $target first"

pkg=target/pkg
rm -rf "$pkg"
mkdir -p "$pkg/man" "$pkg/completions" "$out"
"$bin" man --dir "$pkg/man" > /dev/null
gzip -9n "$pkg"/man/*.1
"$bin" completions bash > "$pkg/completions/stockwise.bash"
"$bin" completions zsh > "$pkg/completions/_stockwise"
"$bin" completions fish > "$pkg/completions/stockwise.fish"

deb="$out/stockwise_$deb_arch.deb"
rpm="$out/stockwise-$rpm_arch.rpm"
cargo deb --no-build --target "$target" --output "$deb"
cargo generate-rpm --target "$target" --output "$rpm"

for file in "$deb" "$rpm"; do
  (
    cd "$(dirname "$file")"
    name=$(basename "$file")
    if command -v sha256sum > /dev/null; then
      sha256sum "$name" > "$name.sha256"
    else
      shasum -a 256 "$name" > "$name.sha256"
    fi
  )
  printf '  %s\n' "$file"
done
