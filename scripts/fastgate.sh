#!/bin/sh
# The gate of the result and provider caches: a warm run prints what a cold run
# prints, byte for byte, and how long each takes.
#
# usage: scripts/fastgate.sh [name=]repo-dir...
#
# Builds the release binary and the plugins, copies each repository to a
# temporary directory (the repository itself is never touched), and on the copy
# runs the strict configuration of scripts/baseline.sh:
#   1. cold with --no-cache, then cold with an empty cache (the write overhead);
#   2. warm with nothing changed;
#   3. warm after each edit of the script: a body-only edit, a rename of a
#      called function, a new caller in another file, a new exported type with
#      a homonym, a rule option, the check of a decision, a deleted file;
# each compared with a --no-cache run of the same tree: the JSON on stdout and
# the exit code must be identical, or the gate fails. It then prints the median
# of 3 for cold, warm without a change, and warm after a body edit of one
# function, phase by phase. It needs no network. Go must be on PATH (GOENV_VERSION
# too, when goenv selects it); the Go build cache should be warm.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
make -s plugins
cargo build -q --release -p lighthouse-cli

python3 - "$root" "$@" <<'PY'
import os, re, shutil, statistics, subprocess, sys, tempfile

root, repos = sys.argv[1], sys.argv[2:]
binary = f"{root}/target/release/lighthouse"
plugins = f"{root}/target/plugins"
RUNS = 3
SKIP = {".git", "target", ".lighthouse", "node_modules", "vendor", ".claude"}
PHASES = ["total", "read", "index", "merge", "hashing", "rules", "identity"]

PROBE = """apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/fastgate-probe
spec:
  title: Probe
  context: A probe of the fast gate.
  scope: {{ subject: symbol }}
  requirement: A function MUST NOT start with a probe letter.
  severity: warn
  check:
    type: cel
    where: symbol.name.startsWith("{prefix}") && symbol.kind == "function"
    message: probe {{{{ symbol.name }}}}
  examples:
    - name: bad
      language: {lang}
      kind: invalid
      files: [{{ path: {path}, body: {bad} }}]
      expect: [{{ line: 3 }}]
    - name: good
      language: {lang}
      kind: valid
      files: [{{ path: {path}, body: {good} }}]
"""


def run(path, cfg, cache):
    args = [binary, "check", ".", "--format", "json", "--timings", "--no-store", "--config", cfg]
    if not cache:
        args.append("--no-cache")
    done = subprocess.run(args, cwd=path, capture_output=True)
    times = {"index": 0.0}
    for line in done.stderr.decode().splitlines():
        m = re.match(r"timings: (.+?) ([0-9.]+)ms(?: \(.*\))?$", line)
        if not m or m.group(1).startswith(("rule ", "analyzer ")):
            continue
        if m.group(1).startswith("index "):
            times["index"] += float(m.group(2))
        else:
            times[m.group(1)] = float(m.group(2))
    return done.returncode, done.stdout, times


def clean(path):
    subprocess.run([binary, "cache", "clean"], cwd=path, capture_output=True)


def median(runs):
    return {p: round(statistics.median(r[p] for r in runs if p in r), 1) for p in PHASES if any(p in r for r in runs)}


def files(path, ext):
    found = []
    for here, dirs, names in os.walk(path):
        dirs[:] = [d for d in dirs if d not in SKIP]
        found += [os.path.relpath(os.path.join(here, n), path) for n in names if n.endswith(ext)]
    return sorted(found)


def read(path, rel):
    with open(f"{path}/{rel}", encoding="utf-8") as f:
        return f.read()


def write(path, rel, text):
    os.makedirs(os.path.dirname(f"{path}/{rel}"), exist_ok=True)
    with open(f"{path}/{rel}", "w", encoding="utf-8") as f:
        f.write(text)


FN = {"go": re.compile(r"^func [^\n]*\{\n", re.M), "rust": re.compile(r"^    (?:pub(?:\([a-z]+\))? )?fn [^\n;]*\{\n", re.M)}
DEF = {"go": r"^func ([a-z][A-Za-z0-9]{5,})\(", "rust": r"^(?:pub(?:\([a-z]+\))? )?fn ([a-z][a-z0-9_]{5,})\("}
STMT = {"go": "\t_ = {n}\n", "rust": "        let _ = {n};\n"}


def sources(path, kind):
    ext = ".go" if kind == "go" else ".rs"
    return [f for f in files(path, ext) if not f.endswith("_test.go") and "/testdata/" not in f and not f.startswith(("vendor/", "tests/"))]


def body_edit(path, rel, kind, n):
    text = read(path, rel)
    m = FN[kind].search(text)
    write(path, rel, text[: m.end()] + STMT[kind].format(n=n) + text[m.end():])


def medium_file(path, kind, avoid=()):
    sized = sorted((len(read(path, f).splitlines()), f) for f in sources(path, kind) if f not in avoid and FN[kind].search(read(path, f)))
    return sized[len(sized) // 2][1]


def rename_word(path, rel, old, new):
    write(path, rel, re.sub(rf"\b{old}\b", new, read(path, rel)))


def called(path, kind):
    """A function defined once, named in at least one file, and the files that name it."""
    texts = {f: read(path, f) for f in sources(path, kind)}
    defs = {}
    for rel, text in texts.items():
        for m in re.finditer(DEF[kind], text, re.M):
            defs.setdefault(m.group(1), set()).add(rel)
    for name in sorted(defs):
        if len(defs[name]) == 1:
            users = [f for f, t in texts.items() if re.search(rf"\b{name}\b", t)]
            home = next(iter(defs[name]))
            if len(users) >= 2 and all(os.path.dirname(u) == os.path.dirname(home) for u in users):
                return name, users
    name = sorted(defs)[0]
    return name, sorted(defs[name])


def script(path, cfg, kind, probe, body_file):
    ext = ".go" if kind == "go" else ".rs"
    name, users = called(path, kind)
    srcs = sources(path, kind)
    other = next(f for f in srcs if os.path.dirname(f) != os.path.dirname(body_file))
    spared = {body_file, other, *users}
    victim = next(f for f in srcs[len(srcs) // 3:] + srcs if f not in spared)
    new = name + "Renamed" if kind == "go" else name + "_renamed"
    caller = users[-1] if users else body_file

    def add_type(rel):
        decl = "\n\n// CacheProbe is a probe.\ntype CacheProbe struct{}\n" if kind == "go" else "\n\n/// A probe.\npub struct CacheProbe;\n"
        write(path, rel, read(path, rel).rstrip("\n") + decl)

    def option():
        with open(cfg, "a") as f:
            f.write('[spec.rules."design/max-name-words"]\nlevel = "warn"\noptions.max = 2\n')

    def new_caller():
        pkg = re.search(r"^package (\w+)", read(path, caller), re.M)
        if kind == "go":
            write(path, os.path.dirname(caller) + "/zz_fastgate.go", f"package {pkg.group(1)}\n\nvar _ = {new}\n")
        else:
            write(path, caller, read(path, caller).rstrip("\n") + f"\n\nfn fastgate_caller() {{\n    let _ = {new};\n}}\n")

    return [
        ("body-only edit", lambda: body_edit(path, body_file, kind, 7)),
        ("rename of a called function", lambda: [rename_word(path, f, name, new) for f in users]),
        ("new caller in another file", new_caller),
        ("new exported type with a homonym", lambda: (add_type(body_file), add_type(other))),
        ("rule option changed", option),
        ("the decision's check edited", lambda: write(path, ".lighthouse/decisions/zz-fastgate-probe.yaml", probe("d"))),
        ("file deleted", lambda: os.remove(f"{path}/{victim}")),
    ]


def main():
    failed = False
    with tempfile.TemporaryDirectory() as scratch:
        for spec in repos:
            label, _, src = spec.rpartition("=")
            src = os.path.abspath(src)
            label = label or os.path.basename(src)
            path = f"{scratch}/{label}"
            subprocess.run(["rsync", "-a", "--exclude", ".git", "--exclude", "target", "--exclude", ".claude", "--exclude", ".lighthouse/cache", "--exclude", ".lighthouse/*.db*", src + "/", path + "/"], check=True)
            kind = "rust" if os.path.exists(f"{path}/Cargo.toml") else "go"
            base = "strict-root" if kind == "rust" else "strict-go"
            text = open(f"{root}/baseline/{base}.toml").read().replace("@PLUGINS@", plugins)
            text = text.replace('plugins = ["core","design","testing",', 'plugins = ["core","design","testing","local",') + '[spec.rules]\n"local/fastgate-probe" = "warn"\n'
            cfg = f"{scratch}/{label}.toml"
            open(cfg, "w").write(text)
            bad, good = ("package a\\n\\nfunc {p}x() {{}}\\n", "package a\\n\\nfunc zz() {{}}\\n") if kind == "go" else ("pub fn {p}x() {{}}\\n", "pub fn zz() {{}}\\n")
            example = ("a.go", "go") if kind == "go" else ("src/lib.rs", "rust")
            probe = lambda p: PROBE.format(prefix=p, lang=example[1], path=example[0], bad='"' + bad.format(p=p) + '"', good='"' + good + '"')
            write(path, ".lighthouse/decisions/zz-fastgate-probe.yaml", probe("s"))

            cold = [run(path, cfg, False) for _ in range(RUNS)]
            if len({(c[0], c[1]) for c in cold}) != 1:
                sys.exit(f"{label}: cold runs disagree")
            first = []
            for _ in range(RUNS):
                clean(path)
                first.append(run(path, cfg, True))
            warm = [run(path, cfg, True) for _ in range(RUNS)]
            for what, runs in (("first run with an empty cache", first), ("warm, nothing changed", warm)):
                for r in runs:
                    if (r[0], r[1]) != (cold[0][0], cold[0][1]):
                        print(f"{label}: {what} differs from cold")
                        failed = True

            body_file = medium_file(path, kind, avoid=called(path, kind)[1])
            steps = script(path, cfg, kind, probe, body_file)
            for step, apply in steps:
                apply()
                expected = run(path, cfg, False)
                got = run(path, cfg, True)
                again = run(path, cfg, True)
                for what, r in (("warm", got), ("warm again", again)):
                    if (r[0], r[1]) != (expected[0], expected[1]):
                        print(f"{label}: {step}: {what} differs from cold")
                        failed = True
                print(f"{label}: {step}: identical={(got[0], got[1]) == (expected[0], expected[1])} total cold {expected[2]['total']}ms warm {got[2]['total']}ms")
            # Medians for the body edit: the edit is applied afresh for each run.
            timed = []
            for n in range(1, RUNS + 1):
                body_edit(path, body_file, kind, 100 + n)
                expected = run(path, cfg, False)
                got = run(path, cfg, True)
                if (got[0], got[1]) != (expected[0], expected[1]):
                    print(f"{label}: body edit {n}: differs from cold")
                    failed = True
                timed.append(got[2])
            cold_t, first_t, warm_t, edit_t = (median([r[2] for r in cold]), median([r[2] for r in first]), median([r[2] for r in warm]), median(timed))
            print(f"\n{label} ({kind}), median of {RUNS}, ms")
            print("| run | " + " | ".join(PHASES) + " |")
            print("| --- | " + " | ".join("---:" for _ in PHASES) + " |")
            for title, t in (("cold --no-cache", cold_t), ("cold, empty cache", first_t), ("warm, no change", warm_t), ("warm, body edit", edit_t)):
                print(f"| {title} | " + " | ".join(str(t.get(p, "")) for p in PHASES) + " |")
            print(f"write overhead of a cold run: {100 * (first_t['total'] / cold_t['total'] - 1):+.0f}%; body edit at {100 * edit_t['total'] / cold_t['total']:.0f}% of cold\n")
    print("FAILED" if failed else "ALL IDENTICAL")
    sys.exit(1 if failed else 0)


main()
PY
