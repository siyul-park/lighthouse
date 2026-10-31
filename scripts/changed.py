#!/usr/bin/env python3
"""What changed against a base ref, as shell assignments for scripts/fast.sh.

usage: scripts/changed.py [base-ref]   (default: main)

The files changed since the merge base (committed, staged, unstaged and
untracked) are mapped to the workspace crates that own them; `crates` is
those and every crate that depends on them (`cargo metadata`). Prints:
  crates=<names>       crates to test and lint
  go=1 typescript=1 python=1   a plugin written in that language changed
  decisions=<ids>      decisions (or `pack/*`) whose files changed
  all_decisions=1      the engine, the checks or a language plugin changed:
                       every decision is tested
"""

import json
import os
import re
import subprocess
import sys

# A change under these decides what every decision example sees.
JUDGES = tuple(
    f"crates/lighthouse-{name}/"
    for name in (
        "engine",
        "checks",
        "spec",
        "model",
        "plugin",
        "rpc",
        "protocol",
        "session",
    )
) + ("plugins/lang-", "protocol/")
# The crates whose tests run the plugins that are not Rust.
SPAWNS = ("lighthouse-checks",)
PLUGIN_DIRS = {
    "go": "plugins/lang-go/",
    "typescript": "plugins/lang-typescript/",
    "python": "plugins/lang-python/",
}


def git(*args):
    """Runs git and returns its output."""
    return subprocess.run(
        ["git", *args], capture_output=True, text=True, check=True
    ).stdout


def changed_files(base):
    """The files changed since the merge base with `base`, untracked ones too."""
    since = git("merge-base", "HEAD", base).strip()
    return set(git("diff", "--name-only", since).split()) | set(
        git("ls-files", "--others", "--exclude-standard").split()
    )


def workspace():
    """The workspace packages, and the directory of each by name."""
    done = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--offline"],
        capture_output=True,
        text=True,
        check=True,
    )
    meta = json.loads(done.stdout)
    root = meta["workspace_root"] + "/"
    members = [p for p in meta["packages"] if p["id"] in meta["workspace_members"]]
    dirs = {
        p["name"]: os.path.dirname(p["manifest_path"]).removeprefix(root) + "/"
        for p in members
    }
    return members, dirs


def affected_crates(files):
    """The crates `files` belong to and every crate that depends on them."""
    members, dirs = workspace()
    if files & {"Cargo.toml", "Cargo.lock"}:
        return sorted(dirs)
    hit = {name for name, d in dirs.items() if any(f.startswith(d) for f in files)}
    if any(f.startswith(d) for f in files for d in PLUGIN_DIRS.values()):
        hit |= set(SPAWNS)
    dependents = {name: set() for name in dirs}
    for package in members:
        for dep in package["dependencies"]:
            if dep["name"] in dependents:
                dependents[dep["name"]].add(package["name"])
    seen, todo = set(hit), list(hit)
    while todo:
        for user in dependents[todo.pop()]:
            if user not in seen:
                seen.add(user)
                todo.append(user)
    return sorted(seen)


def decisions_in(path):
    """The ids a decision file changes: its decisions, or `pack/*` for a pack."""
    ids = []
    with open(path, encoding="utf-8") as f:
        text = f.read()
    for document in re.split(r"^---\s*$", text, flags=re.M):
        kind = re.search(r"^kind:\s*(\w+)", document, re.M)
        name = re.search(r"^metadata:\n(?:[ ]+.*\n)*?[ ]+name:\s*(\S+)", document, re.M)
        if kind and name:
            ids.append(name.group(1) + ("/*" if kind.group(1) == "Pack" else ""))
    return ids


def main():
    """Prints what changed."""
    files = changed_files(sys.argv[1] if len(sys.argv) > 1 else "main")
    out = {"crates": " ".join(affected_crates(files))}
    for language, directory in PLUGIN_DIRS.items():
        out[language] = "1" if any(f.startswith(directory) for f in files) else ""
    judges = any(f.startswith(JUDGES) and "/decisions/" not in f for f in files)
    out["all_decisions"] = "1" if judges else ""
    yamls = [
        f
        for f in files
        if "/decisions/" in f and f.endswith((".yaml", ".yml")) and os.path.exists(f)
    ]
    out["decisions"] = " ".join(sorted({i for f in yamls for i in decisions_in(f)}))
    for key, value in out.items():
        print(f"{key}='{value}'")


if __name__ == "__main__":
    main()
