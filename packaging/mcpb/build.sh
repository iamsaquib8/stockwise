#!/usr/bin/env sh
# Build a Claude Desktop extension (.mcpb) around a StockWise release binary.
#
#   packaging/mcpb/build.sh PATH/TO/stockwise[.exe] TARGET [OUT_DIR]
#
# TARGET names the bundle, e.g. aarch64-apple-darwin -> stockwise-aarch64-apple-darwin.mcpb.
# The version comes from Cargo.toml. A .mcpb is a zip of manifest.json and the
# binary under server/; `npx @anthropic-ai/mcpb validate` checks the manifest.
set -eu

bin=${1:?usage: build.sh PATH/TO/stockwise TARGET [OUT_DIR]}
target=${2:?usage: build.sh PATH/TO/stockwise TARGET [OUT_DIR]}
out=${3:-dist}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)

case "$target" in
  *apple-darwin) platform=darwin ;;
  *windows*) platform=win32 ;;
  *linux*) platform=linux ;;
  *) echo "Unknown target $target" >&2; exit 1 ;;
esac

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/server" "$out"
case "$bin" in
  *.exe) cp "$bin" "$stage/server/stockwise.exe" ;;
  *) cp "$bin" "$stage/server/stockwise"; chmod 755 "$stage/server/stockwise" ;;
esac
# The release version, and only the platform this binary runs on.
python3 - "$here/manifest.json" "$stage/manifest.json" "$version" "$platform" <<'PY'
import json, sys
src, dst, version, platform = sys.argv[1:]
m = json.load(open(src))
m["version"] = version
m["compatibility"]["platforms"] = [platform]
json.dump(m, open(dst, "w"), indent=2, ensure_ascii=False)
PY
cp "$root/LICENSE" "$stage/LICENSE"

bundle="$out/stockwise-$target.mcpb"
rm -f "$bundle"
(cd "$stage" && zip -qr - .) > "$bundle"
echo "$bundle"
shasum -a 256 "$bundle" 2>/dev/null || sha256sum "$bundle"
