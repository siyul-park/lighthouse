"""The Python language provider: the manifest and one `index` request."""

import hashlib
import os
from typing import Any

from lang_python.project import Model
from lang_python.sdk.wire import FileRef, UnitRef, UnitResult
from lang_python.project import is_stub
from lang_python.modules import module_path
from lang_python.project import Project
from lang_python.regions import Remap, Snippet, host_line, plan_snippets
from lang_python.functions import Types
from lang_python import api
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
# The import that stands for every unit: a name is looked up in the whole project.
EVERY = "*"
SETTINGS = ["pyproject.toml"]
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
        "ordered_declarations": True,
        "units": {"by": "file", "settings": SETTINGS},
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
        """Analyzes the files of every unit asked for and the fences of host files. A failure on one file is an
        incomplete entry; a bad request never ends the process."""
        unknown = _unknown_option((params.get("context") or {}).get("options", {}).get(LANGUAGE))
        if unknown is not None:
            return {"units": [], "notices": [], "incomplete": [{"reason": unknown}]}
        return _Indexing(params).run()


class _Indexing:
    """One index request in progress."""

    def __init__(self, params: IndexParams):
        self.root = os.path.realpath(params["project"]["root"])
        context = params.get("context") or {}
        overlays = {o["path"]: o["text"] for o in context.get("overlays") or []}
        self.units = params["units"]
        self.files = [file["path"] for unit in self.units for file in unit.get("files") or []]
        self.hosts: list[EmbeddedFile] = [host for unit in self.units for host in unit.get("regions") or []]
        self.snippets = plan_snippets(self.hosts)
        self.project = Project(
            self.root, overlays, {s.rel: s.text.text for s in self.snippets}, params.get("files") or self.files
        )
        self.model = Model(self.project)
        self.reach = Reach(self.model)
        self.types = Types(self.reach)
        self.incomplete: list[Incomplete] = []
        self.unresolved: dict[str, int] = {}
        self.guessed = 0
        self.notices: list[str] = []

    def run(self) -> IndexResult:
        fenced = self._fences()
        gaps = self.incomplete
        units = [self._unit(unit, fenced, gaps) for unit in self.units]
        return {"units": units, "notices": [], "incomplete": []}

    def _unit(self, unit: UnitRef, fenced: dict[str, Fragment], gaps: list[Incomplete]) -> UnitResult:
        """The answer for one unit: its files, or the fences of its host files."""
        files = unit.get("files") or []
        hosts = unit.get("regions") or []
        self.incomplete = []
        found = [self._file(file["path"]) for file in files]
        fragments = [fragment for fragment in found if fragment is not None]
        fragments.extend(fenced[host["path"]] for host in hosts)
        fragments.sort(key=lambda f: f["file"]["path"])
        notices = [
            "%s: %d relative import(s) resolve to no file; their edges are syntactic" % (path, count)
            for path, count in sorted(self.unresolved.items())
            if path in {f["path"] for f in files}
        ]
        if hosts:
            notices.extend(self.notices)
            if self.guessed:
                notices.append(
                    "%d Python fence(s) use names they do not define or import; those are matched by name (syntactic)"
                    % self.guessed
                )
        hashes = [file["hash"] for file in files] + [host["hash"] for host in hosts]
        body = hashlib.sha256("\n".join(hashes).encode()).hexdigest()
        result: UnitResult = {
            "id": unit["id"],
            "fragments": fragments,
            "body": body,
            "api": "" if hosts else self._api(files),
            "imports": [] if hosts else [EVERY],
            "notices": notices,
            "incomplete": [*self.incomplete, *gaps] if hosts else self.incomplete,
        }
        return result

    def _api(self, files: list[FileRef]) -> str:
        parts = []
        for file in files:
            parsed = self.model.file(file["path"])
            parts.append(file["hash"] if parsed is None else api.digest(parsed.tree, parsed.text))
        return hashlib.sha256("\n".join(parts).encode()).hexdigest()

    def _file(self, rel: str) -> Fragment | None:
        """The fragment of one file; None when the file cannot be read."""
        if is_stub(rel):
            return _empty_fragment(rel)
        parsed = self.model.file(rel)
        if parsed is None:
            self.incomplete.append({"path": rel, "reason": "cannot read the file"})
            return None
        if parsed.tree is None:
            self.incomplete.append({"path": rel, "reason": parsed.error or "cannot parse the file"})
            fragment = _empty_fragment(rel)
            fragment["modules"] = [{"path": module_path(rel)}]
            return fragment
        indexed = self._analyze(rel)
        if indexed is None:
            return _empty_fragment(rel)
        if indexed.unresolved:
            self.unresolved[rel] = indexed.unresolved
        return indexed.fragment

    def _analyze(self, rel: str) -> Indexed | None:
        parsed = self.model.file(rel)
        try:
            return Unit(self.model, self.reach, self.types, parsed).index()
        except RecursionError:
            self.incomplete.append({"path": rel, "reason": "the file is nested too deeply to analyze"})
        except Exception as error:  # one file must not end the batch
            self.incomplete.append({"path": rel, "reason": "internal error: %s: %s" % (type(error).__name__, error)})
        return None

    def _fences(self) -> dict[str, Fragment]:
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
        return merged

    def _fence(self, snippet: Snippet) -> Fragment | None:
        parsed = self.model.file(snippet.rel)
        if parsed is None or parsed.tree is None:
            self.notices.append(
                "%s:%d: the Python fence is not valid Python and is not analysed"
                % (snippet.host["path"], host_line(snippet))
            )
            return None
        indexed = self._analyze(snippet.rel)
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
