"""The Python language provider: the manifest and one `index` request."""

import os
from typing import Any

from lang_python.project import Model
from lang_python.project import is_ignored, is_stub
from lang_python.modules import module_path
from lang_python.project import Project
from lang_python.regions import Remap, Snippet, host_line, plan_snippets
from lang_python.functions import Types
from lang_python.index import Indexed, Unit
from lang_python.visibility import Reach
from lang_python.sdk.wire import (
    EmbeddedFile,
    Fragment,
    Incomplete,
    IndexParams,
    IndexResult,
    LanguageManifest,
)

LANGUAGE = "python"
# GitHub Linguist's name and aliases for Python (lib/linguist/languages.yml).
ALIASES = ["python", "py", "py3", "python3", "rusthon"]
SOURCE_GLOB = "**/*.{py,pyi}"
TEST_GLOBS = ["**/test_*.py", "**/*_test.py", "**/conftest.py", "**/tests/**", "**/test/**"]
CONSTRUCTOR_PREFIXES = ["new", "create", "from"]
CAPABILITIES = ["semantic-edges", "extent", "reference-sites", "overlays"]


def language() -> LanguageManifest:
    """The manifest of Python: its globs, aliases, test conventions and capabilities."""
    return {
        "id": LANGUAGE,
        "aliases": ALIASES,
        "globs": [SOURCE_GLOB],
        "priority": 0,
        "conventions": {
            "test_globs": TEST_GLOBS,
            "constructor_prefixes": CONSTRUCTOR_PREFIXES,
            "test_naming": {"prefix": "test_", "snake_case": True, "variants": False, "ancestors": False},
        },
        "capabilities": CAPABILITIES,
    }


def _empty_fragment(rel: str) -> Fragment:
    return {
        "file": {"path": rel},
        "modules": [],
        "symbols": [],
        "edges": [],
        "functions": [],
        "tests": [],
        "comments": [],
    }


class Provider:
    """The language provider of Python: it implements the SDK's `Provider`."""

    def __init__(self, name: str, version: str):
        self.name = name
        self.version = version

    def manifest(self) -> list[LanguageManifest]:
        """The language manifests the provider answers with."""
        return [language()]

    def index(self, params: IndexParams) -> IndexResult:
        """Analyzes every requested file and the fences of host files. A failure on one file is an incomplete
        entry; a bad request never ends the process."""
        unknown = _unknown_option((params.get("context") or {}).get("options", {}).get(LANGUAGE))
        if unknown is not None:
            return {"fragments": [], "sources": [], "notices": [], "incomplete": [{"reason": unknown}]}
        run = _Indexing(params)
        return run.run()


class _Indexing:
    """One index request in progress."""

    def __init__(self, params: IndexParams):
        self.root = os.path.realpath(params["project"]["root"])
        context = params.get("context") or {}
        overlays = {o["path"]: o["text"] for o in context.get("overlays") or []}
        self.files: list[str] = []
        self.hosts: list[EmbeddedFile] = []
        for item in params["inputs"]:
            if item["kind"] == "file":
                self.files.append(item["file"]["path"])
            else:
                self.hosts.append(item["regions"])
        self.snippets = plan_snippets(self.hosts)
        self.project = Project(self.root, overlays, {s.rel: s.text.text for s in self.snippets})
        self.model = Model(self.project)
        self.reach = Reach(self.model)
        self.types = Types(self.reach)
        self.fragments: list[Fragment] = []
        self.incomplete: list[Incomplete] = []
        self.notices: list[str] = []
        self.unresolved = 0
        self.guessed = 0

    def run(self) -> IndexResult:
        for rel in dict.fromkeys(self.files):
            self._file(rel)
        for host, fragment in self._fences():
            self.fragments.append(fragment)
        self.fragments.sort(key=lambda f: f["file"]["path"])
        if self.unresolved:
            self.notices.append("%d relative import(s) resolve to no file; their edges are syntactic" % self.unresolved)
        if self.guessed:
            self.notices.append(
                "%d Python fence(s) use names they do not define or import; those are matched by name (syntactic)"
                % self.guessed
            )
        return {"fragments": self.fragments, "sources": [], "notices": self.notices, "incomplete": self.incomplete}

    def _file(self, rel: str) -> None:
        if is_ignored(rel) or is_stub(rel):
            self.fragments.append(_empty_fragment(rel))
            return
        parsed = self.model.file(rel)
        if parsed is None:
            self.incomplete.append({"path": rel, "reason": "cannot read the file"})
            return
        if parsed.tree is None:
            self.incomplete.append({"path": rel, "reason": parsed.error or "cannot parse the file"})
            fragment = _empty_fragment(rel)
            fragment["modules"] = [{"path": module_path(rel)}]
            self.fragments.append(fragment)
            return
        indexed = self._unit(rel)
        if indexed is None:
            self.fragments.append(_empty_fragment(rel))
            return
        self.unresolved += indexed.unresolved
        self.fragments.append(indexed.fragment)

    def _unit(self, rel: str) -> Indexed | None:
        parsed = self.model.file(rel)
        try:
            return Unit(self.model, self.reach, self.types, parsed).index()
        except RecursionError:
            self.incomplete.append({"path": rel, "reason": "the file is nested too deeply to analyze"})
        except Exception as error:  # one file must not end the batch
            self.incomplete.append({"path": rel, "reason": "internal error: %s: %s" % (type(error).__name__, error)})
        return None

    def _fences(self) -> list[tuple[EmbeddedFile, Fragment]]:
        """The fragments of the host files: the facts of their fences, in the host's coordinates."""
        merged: dict[str, Fragment] = {}
        for host in self.hosts:
            merged[host["path"]] = _empty_fragment(host["path"])
        for snippet in self.snippets:
            fragment = self._fence(snippet)
            if fragment is None:
                continue
            target = merged[snippet.host["path"]]
            for key in ("modules", "symbols", "edges", "functions", "tests", "comments"):
                target[key].extend(fragment.get(key, []))  # type: ignore[literal-required]
        return [(host, merged[host["path"]]) for host in self.hosts]

    def _fence(self, snippet: Snippet) -> Fragment | None:
        parsed = self.model.file(snippet.rel)
        if parsed is None or parsed.tree is None:
            self.notices.append(
                "%s:%d: the Python fence is not valid Python and is not analysed"
                % (snippet.host["path"], host_line(snippet))
            )
            return None
        indexed = self._unit(snippet.rel)
        if indexed is None:
            return None
        if indexed.guessed:
            self.guessed += 1
        module = indexed.fragment["modules"][0]["path"]
        return Remap(snippet, module).fragment(indexed.fragment)


def _unknown_option(options: Any) -> str | None:
    """The reason the provider's options are refused, if they are: it has none yet, so any key is unknown."""
    if options is None:
        return None
    if not isinstance(options, dict):
        return "[languages.%s] must be a table" % LANGUAGE
    for key in options:
        return "unknown key `%s` in [languages.%s]" % (key, LANGUAGE)
    return None
