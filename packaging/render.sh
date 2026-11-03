#!/bin/sh
# Render the package-manager templates in packaging/ for one release.
#
#   gh release download v0.8.0 --pattern '*.sha256' --dir sums
#   packaging/render.sh 0.8.0 sums dist
#
# Fills @VERSION@ and @SHA256_<TARGET>@ from the release's .sha256 files (asset
# names from .github/workflows/release.yml), drops the "#:" template comments,
# and writes:
#
#   dist/homebrew/stockwise.rb                 -> Formula/ in iamsaquib8/homebrew-tap
#   dist/scoop/stockwise.json                  -> a Scoop bucket, or attached to the release
#   dist/winget/manifests/i/iamsaquib8/...     -> a PR to microsoft/winget-pkgs
#   dist/aur/stockwise-bin/{PKGBUILD,.SRCINFO} -> the stockwise-bin AUR repository
set -eu

die() { printf 'render.sh: %s\n' "$*" >&2; exit 1; }

[ $# -eq 3 ] || die "usage: render.sh VERSION SUMS_DIR OUT_DIR"
version=${1#v}
sums=$2
out=$3
here=$(cd "$(dirname "$0")" && pwd)

printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$' ||
  die "'$version' is not a version like 0.3.0"
[ -d "$sums" ] || die "no such directory: $sums"
mkdir -p "$out"

script="$out/.render.sed"
trap 'rm -f "$script"' EXIT INT TERM
{
  echo '/^#:/d'
  printf 's|@VERSION@|%s|g\n' "$version"
} > "$script"

for target in \
  x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
  x86_64-apple-darwin aarch64-apple-darwin \
  x86_64-pc-windows-msvc; do
  case "$target" in
    *windows*) asset="stockwise-$target.zip" ;;
    *) asset="stockwise-$target.tar.gz" ;;
  esac
  file="$sums/$asset.sha256"
  [ -f "$file" ] || die "missing $file"
  # `sha256sum` / `shasum -a 256` output: "<hash>  <file>" (or " *<file>").
  hash=$(cut -d ' ' -f 1 < "$file" | tr 'A-F' 'a-f')
  printf '%s' "$hash" | grep -Eq '^[0-9a-f]{64}$' || die "no SHA-256 in $file"
  key=$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')
  printf 's|@SHA256_%s@|%s|g\n' "$key" "$hash" >> "$script"
done

render() {
  mkdir -p "$(dirname "$2")"
  sed -f "$script" "$1" > "$2"
  if grep -n '@[A-Z0-9_]*@' "$2" >&2; then
    die "unfilled placeholder in $2"
  fi
  printf '  %s\n' "$2"
}

echo "Rendered StockWise $version:"
render "$here/homebrew/stockwise.rb" "$out/homebrew/stockwise.rb"
render "$here/scoop/stockwise.json" "$out/scoop/stockwise.json"
winget="$out/winget/manifests/i/iamsaquib8/StockWise/$version"
for manifest in "$here"/winget/*.yaml; do
  render "$manifest" "$winget/$(basename "$manifest")"
done
render "$here/aur/stockwise-bin/PKGBUILD" "$out/aur/stockwise-bin/PKGBUILD"
render "$here/aur/stockwise-bin/.SRCINFO" "$out/aur/stockwise-bin/.SRCINFO"
