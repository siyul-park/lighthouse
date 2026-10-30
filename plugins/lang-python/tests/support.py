"""Helpers of the tests: a project written to a temporary directory and indexed through the provider."""

import os
import tempfile
from typing import Any

from lang_python.provider import Provider


def write_project(root: str, files: dict[str, str]) -> None:
    for rel, text in files.items():
        path = os.path.join(root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)


def request(root: str, paths: list[str], regions: list[Any] | None = None, options: Any = None) -> dict[str, Any]:
    inputs: list[Any] = [{"kind": "file", "file": {"path": p, "hash": "0"}} for p in paths]
    inputs += [{"kind": "regions", "regions": r} for r in regions or []]
    context: dict[str, Any] = {"options": {} if options is None else {"python": options}}
    return {"project": {"root": root}, "language": "python", "inputs": inputs, "context": context}


def index(files: dict[str, str], paths: list[str] | None = None, regions: list[Any] | None = None, options: Any = None):
    """Indexes a project of in-memory files; the requested paths default to all of them."""
    with tempfile.TemporaryDirectory() as root:
        write_project(root, files)
        wanted = sorted(files) if paths is None else paths
        return Provider("lang-python", "0").index(request(root, wanted, regions, options))  # type: ignore[arg-type]


def fragment(result: dict[str, Any], path: str) -> dict[str, Any]:
    return next(f for f in result["fragments"] if f["file"]["path"] == path)


def symbols(result: dict[str, Any], path: str) -> dict[str, dict[str, Any]]:
    return {s["id"]: s for s in fragment(result, path)["symbols"]}


def edges(result: dict[str, Any], path: str, kind: str | None = None) -> list[tuple[str, str, str, str]]:
    found = []
    for e in fragment(result, path)["edges"]:
        if kind is not None and e["kind"] != kind:
            continue
        source = e["from"].get("symbol") or e["from"]["module"]
        found.append((e["kind"], source, e["to"], e["resolution"]))
    return found
