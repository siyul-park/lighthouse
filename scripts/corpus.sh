#!/bin/sh
# The corpus gate: a base binary and the tip binary find the same things in the
# same repositories, or the difference is shown.
#
# usage: scripts/corpus.sh [--base BINARY] [--tip BINARY] [--plugins DIR]
#                          [--base-plugins DIR] [--no-build] [name=]repo-dir...
#
#   --base     the binary to compare against (a release build of main). Without
#              it the run only checks the tip and prints its findings.
#   --tip      default target/release/lighthouse, built (with the plugins) by
#              `make plugins cli` unless --no-build.
#   --plugins  the plugin directory the tip binary uses, default target/plugins.
#   --base-plugins  the plugin directory the base binary uses, default --plugins.
#   --keep     keep each corpus's findings (<DIR>/<name>/{base,tip}.json) instead
#              of a temporary directory removed on exit.
#              The base runs with the plugins built from its own commit, so a
#              lang-* change is compared honestly. The usual call: a release
#              build of main, its `target/plugins` copied to a temp directory,
#              `--base main/lighthouse --base-plugins main/plugins`.
#   repo-dir   default: cel, qlbridge (under $CORPUS_HOME, default ~/Projects)
#              and this repository's HEAD. Each is copied out with
#              `git archive HEAD`, never touched; what the analysis of a
#              corpus cannot load is removed from the copy (cel: codelab/,
#              which is no Go module of the build, conformance/ and repl/,
#              which need the Bazel build's inputs, and tools/celtest/ with
#              policy/test/, its only importer, which build against another
#              revision of cel's policy package), so
#              that the analysis is complete and `fix` does not refuse it.
#
# Each corpus is checked by both binaries with scripts/baseline/strict-go.toml
# (strict-root.toml for a Cargo workspace), `--format json --detail full --limit 1000000`,
# no cache and no store. The corpora run in parallel; the summaries print in
# the order given: per repo the findings of each binary, the findings only one
# has (by fingerprint, then by decision), and the exit codes. It exits 1 when
# any corpus differs. Everything it makes lives in one temporary directory,
# removed on exit.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
here=$PWD
cd "$root"
# A path given relative is relative to where the script was run from.
absolute() {
    case $1 in /*) echo "$1" ;; *) echo "$here/$1" ;; esac
}
keep="" base="" base_plugins="" tip="$root/target/release/lighthouse" plugins="$root/target/plugins" build=1
while [ $# -gt 0 ]; do
    case "$1" in
    --base) base=$2; shift 2 ;;
    --tip) tip=$2; shift 2 ;;
    --plugins) plugins=$2; shift 2 ;;
    --base-plugins) base_plugins=$2; shift 2 ;;
    --no-build) build=0; shift ;;
    --keep) keep=$2; shift 2 ;;
    --) shift; break ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) break ;;
    esac
done
[ -z "$base" ] || base=$(absolute "$base")
tip=$(absolute "$tip")
plugins=$(absolute "$plugins")
[ -n "$base_plugins" ] || base_plugins=$plugins
base_plugins=$(absolute "$base_plugins")
if [ "$build" = 1 ]; then
    make -s plugins cli
fi
home=${CORPUS_HOME:-$HOME/Projects}
[ $# -gt 0 ] || set -- "cel=$home/cel" "qlbridge=$home/qlbridge" "lighthouse=$root"

if [ -n "$keep" ]; then
    scratch=$(absolute "$keep")
    mkdir -p "$scratch"
else
    scratch=$(mktemp -d "${TMPDIR:-/tmp}/lighthouse-corpus.XXXXXX")
    trap 'rm -rf "$scratch"' EXIT
fi
trap 'exit 130' INT TERM
# An installed plugin must not shadow the ones named in the configuration.
mkdir "$scratch/xdg"
export XDG_DATA_HOME="$scratch/xdg"

# Checks one corpus with one binary: findings in $dir/<tag>.json, the exit code
# in $dir/<tag>.rc. The tree is a scratch archive checked with the gate's own
# configuration, so this run trusts it.
check() {
    dir=$1 tag=$2 binary=$3 config=$4
    (cd "$dir/tree" && LIGHTHOUSE_TRUST=1 "$binary" check . --format json --detail full --limit 1000000 --no-cache --no-store --config "$dir/$config" >"$dir/$tag.json" 2>"$dir/$tag.err" && echo 0 >"$dir/$tag.rc") ||
        echo "$?" >"$dir/$tag.rc"
}

corpus() {
    spec=$1
    label=${spec%%=*}
    src=${spec#*=}
    [ "$label" != "$spec" ] || label=$(basename "$src")
    dir="$scratch/$label"
    mkdir -p "$dir/tree"
    git -C "$src" archive HEAD | tar -x -C "$dir/tree"
    rm -rf "$dir/tree/codelab"
    case $label in
    cel) rm -rf "$dir/tree/conformance" "$dir/tree/repl" "$dir/tree/tools/celtest" "$dir/tree/policy/test" ;;
    esac
    preset=strict-go
    [ ! -f "$dir/tree/Cargo.toml" ] || preset=strict-root
    sed "s|@PLUGINS@|$plugins|g" "scripts/baseline/$preset.toml" >"$dir/config.toml"
    sed "s|@PLUGINS@|$base_plugins|g" "scripts/baseline/$preset.toml" >"$dir/base-config.toml"
    [ -z "$base" ] || check "$dir" base "$base" base-config.toml
    check "$dir" tip "$tip" config.toml
    python3 -I scripts/corpus-diff.py "$label" "$dir" "$([ -n "$base" ] && echo 1)" >"$dir/summary.txt" || echo 1 >"$dir/differs"
}

for spec in "$@"; do
    corpus "$spec" &
done
wait

status=0
for spec in "$@"; do
    label=${spec%%=*}
    [ "$label" != "$spec" ] || label=$(basename "${spec#*=}")
    cat "$scratch/$label/summary.txt"
    [ ! -f "$scratch/$label/differs" ] || status=1
done
if [ "$status" = 0 ]; then echo "ALL IDENTICAL"; else echo "DIFFERENT"; fi
exit "$status"
