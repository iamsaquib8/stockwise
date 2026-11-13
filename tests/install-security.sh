#!/bin/sh
# Offline regression checks for the curl | sh installer's fail-closed paths.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

case "$(uname -s)" in
  Linux) platform=unknown-linux-gnu ;;
  Darwin) platform=apple-darwin ;;
  *) echo 'unsupported test platform' >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) echo 'unsupported test architecture' >&2; exit 1 ;;
esac

target="${arch}-${platform}"
asset="stockwise-${target}.tar.gz"
mkdir -p "$work/package/stockwise-${target}" "$work/bin"
printf '#!/bin/sh\nprintf "stockwise test\\n"\n' > "$work/package/stockwise-${target}/stockwise"
chmod +x "$work/package/stockwise-${target}/stockwise"
(cd "$work/package" && tar czf "$work/archive.tar.gz" "stockwise-${target}")
if command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum "$work/archive.tar.gz" | cut -d ' ' -f 1)
else
  digest=$(shasum -a 256 "$work/archive.tar.gz" | cut -d ' ' -f 1)
fi
printf '%s  %s\n' "$digest" "$asset" > "$work/archive.sha256"

# The fake curl never opens a network connection.
printf '%s\n' '#!/bin/sh' 'set -eu' \
  'url=$2' 'dest=$4' \
  'case "$url" in' \
  '  *.sha256)' \
  '    [ "${MOCK_MISSING_CHECKSUM:-0}" != 1 ] || exit 22' \
  '    cp "$MOCK_CHECKSUM" "$dest" ;;' \
  '  *.tar.gz) cp "$MOCK_ARCHIVE" "$dest" ;;' \
  '  *) exit 22 ;;' \
  'esac' > "$work/bin/curl"
chmod +x "$work/bin/curl"

export MOCK_ARCHIVE="$work/archive.tar.gz" MOCK_CHECKSUM="$work/archive.sha256"
PATH="$work/bin:$PATH"
export PATH

if MOCK_MISSING_CHECKSUM=1 STOCKWISE_INSTALL="$work/rejected" sh "$root/install.sh" > "$work/rejected.log" 2>&1; then
  echo 'installer accepted a missing checksum' >&2
  exit 1
fi
test ! -e "$work/rejected/stockwise"
grep -q "checksum file not found for $asset" "$work/rejected.log"

MOCK_MISSING_CHECKSUM=1 STOCKWISE_SKIP_CHECKSUM=1 STOCKWISE_INSTALL="$work/unsafe" sh "$root/install.sh" > "$work/unsafe.log" 2>&1
test -x "$work/unsafe/stockwise"
grep -q 'checksum verification explicitly disabled' "$work/unsafe.log"

STOCKWISE_INSTALL="$work/verified" sh "$root/install.sh" > "$work/verified.log" 2>&1
test -x "$work/verified/stockwise"
grep -q 'Checksum verified' "$work/verified.log"

# Emulate a pipe cut off before the final main call: no installation occurs.
sed '$d' "$root/install.sh" | STOCKWISE_INSTALL="$work/truncated" sh
test ! -e "$work/truncated/stockwise"
echo 'installer security regression checks passed'
