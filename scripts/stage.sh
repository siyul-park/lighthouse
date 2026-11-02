#!/bin/sh
# Stages the release archive of a built tree.
#
# usage: scripts/stage.sh <version> <target> <out-dir>
#
# Run after `make plugins cli`. Writes <out-dir>/lighthouse-<version>-<target>/
# (the executable, plugins/, README.md and LICENSE when there is one) and its
# tarball beside it.
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: scripts/stage.sh <version> <target> <out-dir>" >&2
    exit 2
fi
version=$1
target=$2
out=$3
name=lighthouse-$version-$target

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
rm -rf "${out:?}/$name" "$out/$name.tar.gz"
mkdir -p "$out/$name/plugins"
cp target/release/lighthouse "$out/$name/lighthouse"
for plugin in target/plugins/*/; do
    cp -R "$plugin" "$out/$name/plugins/$(basename "$plugin")"
done
cp README.md "$out/$name/"
if [ -f LICENSE ]; then
    cp LICENSE "$out/$name/"
fi
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"
