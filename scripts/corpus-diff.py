"""The summary of one corpus of scripts/corpus.sh: what the base and the tip binary found.

usage: corpus-diff.py LABEL DIR [COMPARE]

DIR holds base.json and tip.json (`check --format json`) with their .rc and
.err files; COMPARE is non-empty when a base was run. Prints the summary and
exits 1 when the two differ.
"""

import collections
import json
import sys


def load(directory, tag):
    """The report, the exit code and the stderr of one binary."""
    with open(f"{directory}/{tag}.rc", encoding="utf-8") as f:
        code = f.read().strip()
    with open(f"{directory}/{tag}.err", encoding="utf-8") as f:
        err = f.read().strip()
    try:
        with open(f"{directory}/{tag}.json", encoding="utf-8") as f:
            return json.load(f), code, err
    except (OSError, json.JSONDecodeError):
        return None, code, err


def describe(tag, report, code, err):
    """One line for a binary: its findings, status and exit code."""
    if report is None:
        return [
            f"  {tag}: no JSON (exit {code}): {err.splitlines()[-1] if err else 'no output'}"
        ]
    return [
        f"  {tag}: {len(report['findings'])} findings, status {report['status']}, exit {code}"
    ]


def compare(base, tip):
    """The lines saying how two reports differ, and whether they do."""
    only_base = {f["fingerprint"]: f for f in base[0]["findings"]}
    only_tip = {f["fingerprint"]: f for f in tip[0]["findings"]}
    for key in only_base.keys() & only_tip.keys():
        del only_base[key], only_tip[key]
    same = len(base[0]["findings"]) - len(only_base)
    lines = [
        f"  only in base: {len(only_base)}, only in tip: {len(only_tip)}, same: {same}"
    ]
    delta = collections.Counter(f["decision"] for f in only_tip.values())
    delta.subtract(collections.Counter(f["decision"] for f in only_base.values()))
    lines += [f"    {decision}: {n:+d}" for decision, n in sorted(delta.items())]
    if base[1] != tip[1]:
        lines.append(f"  exit codes differ: base {base[1]}, tip {tip[1]}")
    differs = (
        bool(only_base or only_tip)
        or base[1] != tip[1]
        or base[0]["status"] != tip[0]["status"]
    )
    return lines, differs


def main():
    """Prints the summary of one corpus."""
    label, directory = sys.argv[1], sys.argv[2]
    tip = load(directory, "tip")
    lines = [label, *describe("tip", *tip)]
    differs = tip[0] is None
    if len(sys.argv) > 3 and sys.argv[3]:
        base = load(directory, "base")
        lines = [label, *describe("base", *base), *lines[1:]]
        if base[0] is None or tip[0] is None:
            differs = True
        else:
            more, differs = compare(base, tip)
            lines += more + ["  DIFFERENT" if differs else "  IDENTICAL"]
    print("\n".join(lines))
    sys.exit(1 if differs else 0)


if __name__ == "__main__":
    main()
