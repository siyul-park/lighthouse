#!/bin/sh
# Fixes the numbers that search, learned checks and caches are judged against.
#
# usage: scripts/baseline.sh [name=]repo-dir...
#
# Builds the release binary and the plugins, then checks each repository with
# the strict configuration (baseline/strict-root.toml for a Cargo workspace,
# baseline/strict-go.toml for a Go module), three times, and writes
# baseline/<name>.json: the commit, the size, the exit code, the findings per
# decision and the median time of each phase. Everything but the times is
# deterministic. It needs no network. Go must be on PATH (GOENV_VERSION too,
# when goenv selects it); the Go build cache should be warm.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
make -s plugins
cargo build -q --release -p lighthouse-cli

python3 - "$root" "$@" <<'PY'
import hashlib, json, os, statistics, subprocess, sys, re

root, repos = sys.argv[1], sys.argv[2:]
binary = f"{root}/target/release/lighthouse"
plugins = f"{root}/target/plugins"
RUNS = 3
SOURCES = {".go": "go", ".rs": "rust"}
SKIP = {".git", "target", ".lighthouse", "node_modules", "vendor"}


def sources(path):
    counts = {}
    for here, dirs, names in os.walk(path):
        dirs[:] = [d for d in dirs if d not in SKIP]
        for name in names:
            lang = SOURCES.get(os.path.splitext(name)[1])
            if lang:
                with open(os.path.join(here, name), "rb") as f:
                    lines = f.read().count(b"\n")
                entry = counts.setdefault(lang, {"files": 0, "lines": 0})
                entry["files"] += 1
                entry["lines"] += lines
    return counts


def commit(path):
    """The commit, and what differs from it: the files and a hash of the diff."""
    git = lambda *a: subprocess.run(["git", "-C", path, *a], capture_output=True, text=True)
    head = git("rev-parse", "HEAD")
    if head.returncode:
        # A copy without history names its commit in a file of its own.
        marker = f"{path}/.baseline-commit"
        return (open(marker).read().strip() if os.path.exists(marker) else None), [], None
    dirty = sorted(line[3:] for line in git("status", "--short").stdout.splitlines())
    # Tracked changes by content; untracked files are named in the list.
    diff = hashlib.sha256(git("diff", "HEAD").stdout.encode()).hexdigest()
    return head.stdout.strip(), dirty, diff


def config(path, directory):
    name = "strict-root" if os.path.exists(f"{path}/Cargo.toml") else "strict-go"
    text = open(f"{root}/baseline/{name}.toml").read().replace("@PLUGINS@", plugins)
    out = f"{directory}/{name}.toml"
    open(out, "w").write(text)
    return name, out


def run(path, cfg):
    done = subprocess.run(
        [binary, "check", ".", "--format", "json", "--timings", "--no-store", "--config", cfg],
        cwd=path, capture_output=True, text=True,
    )
    counts, incomplete = {}, 0
    for line in done.stdout.splitlines():
        if not line.startswith("{"):
            continue
        found = json.loads(line)
        if "ruleId" in found:
            counts[found["ruleId"]] = counts.get(found["ruleId"], 0) + 1
        else:
            incomplete += 1
    times = {}
    for line in done.stderr.splitlines():
        m = re.match(r"timings: (.+?) ([0-9.]+)ms(?: \(.*\))?$", line)
        if m and not m.group(1).startswith("rule "):
            times[m.group(1)] = float(m.group(2))
    return done.returncode, counts, incomplete, times


import tempfile
with tempfile.TemporaryDirectory() as scratch:
    for spec in repos:
        name, _, path = spec.rpartition("=")
        path = os.path.abspath(path)
        name = name or os.path.basename(path)
        cfg_name, cfg = config(path, scratch)
        results = [run(path, cfg) for _ in range(RUNS)]
        if len({(r[0], json.dumps(r[1], sort_keys=True), r[2]) for r in results}) != 1:
            sys.exit(f"{name}: the runs disagree on exit code or findings")
        code, counts, incomplete, _ = results[0]
        phases = {p: round(statistics.median(r[3][p] for r in results), 1) for p in results[0][3]}
        head, dirty, diff = commit(path)
        report = {
            "repo": name,
            "commit": head,
            "dirtyFiles": dirty,
            "diffSha256": diff,
            "config": cfg_name,
            "sources": sources(path),
            "exitCode": code,
            "incomplete": incomplete,
            "findings": dict(sorted(counts.items())),
            "findingsTotal": sum(counts.values()),
            "timingsMs": {"runs": RUNS, "median": phases},
        }
        with open(f"{root}/baseline/{name}.json", "w") as out:
            json.dump(report, out, indent=2, sort_keys=True)
            out.write("\n")
        print(f"{name}: {sum(counts.values())} findings, exit {code}, total {phases.get('total')}ms")
PY
