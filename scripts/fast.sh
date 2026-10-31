#!/bin/sh
# The agent's gate after every step: what changed against BASE (default main)
# and what depends on it. `make ci` is the full gate, run before a merge.
#
# usage: scripts/fast.sh        (or: make fast [BASE=ref])
#
#   - fmt; clippy and `cargo test` of the changed crates and their dependents;
#   - the Go, TypeScript and Python checks only when that plugin changed;
#   - the dogfood check, and the generated docs;
#   - `decision test` of the decisions whose files changed, or of all of them
#     when the engine, the checks, the spec or a language plugin changed.
set -efu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
export LIGHTHOUSE_TRUST=1
lighthouse=target/release/lighthouse

step() {
    printf '== %s\n' "$*"
    started=$(date +%s)
    "$@"
    printf '   %s s\n' "$(($(date +%s) - started))"
}

eval "$(python3 scripts/changed.py "${BASE:-main}")"
printf 'changed: crates=[%s] go=%s typescript=%s python=%s decisions=[%s] all_decisions=%s\n' \
    "$crates" "$go" "$typescript" "$python" "$decisions" "$all_decisions"

step cargo fmt --all --check
if [ -n "$crates" ]; then
    packages=""
    for crate in $crates; do packages="$packages -p $crate"; done
    # shellcheck disable=SC2086
    step cargo clippy $packages --all-targets -- -D warnings
    # shellcheck disable=SC2086
    step cargo test $packages
fi
if [ -n "$go" ]; then
    step sh -c 'cd plugins/lang-go && test -z "$(gofmt -l .)" && go vet ./... && go test ./... && cd sdk && go run ./gen -check'
fi
if [ -n "$typescript" ]; then
    step sh -c 'cd plugins/lang-typescript && npm test'
fi
if [ -n "$python" ]; then
    step sh -c 'cd plugins/lang-python && ruff format --check . && ruff check . && python3 -I scripts/wire.py --check && python3 -I -m unittest discover -s tests -t .'
fi

step make -s plugins cli
step "$lighthouse" check . --allow-incomplete
if [ -n "$all_decisions" ]; then
    step "$lighthouse" decision test
elif [ -n "$decisions" ]; then
    # shellcheck disable=SC2086
    step "$lighthouse" decision test $decisions
fi
step "$lighthouse" docs check
echo "fast gate passed"
