#!/usr/bin/env bash
# Packages one release build as the releases always were:
#   OUT/linearctl_VERSION_GOOS_GOARCH.tar.gz (.zip for Windows), the binary,
#   LICENSE, README.md and CHANGELOG.md at its root; and
#   OUT/linearctl_GOOS_GOARCH/linearctl[.exe], which npm-packages.mjs takes.
#
#   scripts/package.sh TARGET GOOS GOARCH VERSION OUT
set -euo pipefail
target=$1 goos=$2 goarch=$3 version=$4 out=$5
root=$(cd "$(dirname "$0")/.." && pwd)
exe=linearctl
[ "$goos" = windows ] && exe=linearctl.exe
bin=$root/target/$target/release/$exe
[ -f "$bin" ] || { echo "no build at $bin" >&2; exit 1; }

mkdir -p "$out/linearctl_${goos}_${goarch}"
cp "$bin" "$out/linearctl_${goos}_${goarch}/$exe"

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp "$bin" "$root/LICENSE" "$root/README.md" "$root/CHANGELOG.md" "$stage/"
name=linearctl_${version}_${goos}_${goarch}
out=$(cd "$out" && pwd)
if [ "$goos" = windows ]; then
  (cd "$stage" && 7z a -tzip -bso0 "$out/$name.zip" ./*)
else
  tar -C "$stage" -czf "$out/$name.tar.gz" "$exe" LICENSE README.md CHANGELOG.md
fi
echo "packaged $name"
