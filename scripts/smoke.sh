#!/bin/sh
# The smoke test of an unpacked release archive.
#
# usage: scripts/smoke.sh <unpacked dir> <version>
#
# Checks that the executable prints the version, that every plugin the fixture
# lists is installed next to it and found, and that `lighthouse check` over a
# file of each language exits 0 or 1, never 2 (usage, runtime or plugin
# error) or 3 (incomplete analysis). The project is trusted for this run only,
# as CI trusts it (docs/install.md).
set -efu

if [ "$#" -ne 2 ]; then
    echo "usage: scripts/smoke.sh <unpacked dir> <version>" >&2
    exit 2
fi
dir=$(cd "$1" && pwd)
version=$2
fixture=$(cd "$(dirname "$0")/smoke" && pwd)
exe=$dir/lighthouse

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp -R "$fixture/." "$work/project"
export XDG_CONFIG_HOME=$work/home XDG_DATA_HOME=$work/home XDG_CACHE_HOME=$work/home

printed=$("$exe" --version)
if [ "$printed" != "lighthouse $version" ]; then
    echo "smoke: --version printed '$printed', expected 'lighthouse $version'" >&2
    exit 1
fi
echo "version: $printed"

# `check` fails on a listed plugin it does not find, which is the discovery
# test; the manifest is checked first to name what is missing.
plugins=$(sed -n 's/^  "\(.*\)",$/\1/p' "$work/project/lighthouse.toml")
for plugin in $plugins; do
    if [ ! -f "$dir/plugins/$plugin/lighthouse-plugin.toml" ]; then
        echo "smoke: plugin $plugin is not installed next to the executable" >&2
        exit 1
    fi
    echo "plugin: $plugin"
done

status=0
(cd "$work/project" && LIGHTHOUSE_TRUST=1 "$exe" check) || status=$?
echo "check: exit $status"
if [ "$status" -gt 1 ]; then
    echo "smoke: check exited $status, expected 0 or 1" >&2
    exit 1
fi
echo "smoke passed"
