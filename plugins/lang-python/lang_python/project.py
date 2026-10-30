"""The project on disk: its files, its import roots and how a dotted module name resolves to a file."""

import os
import posixpath
import sys
import tokenize
import tomllib
from dataclasses import dataclass
from typing import Any

from lang_python.paths import IGNORED_DIRS, SOURCE_SUFFIXES, is_ignored, module_path

STDLIB = frozenset(sys.stdlib_module_names)


@dataclass(frozen=True)
class ModuleLoc:
    """A module a dotted name resolves to inside the project."""

    module: str
    """The module path (a file without extension; a package is its directory)."""
    rel: str | None
    """The file that declares it; None for a namespace package (PEP 420)."""
    is_package: bool


class Project:
    """The files of a project, as the request sees them: overlays stand in for files on disk."""

    def __init__(self, root: str, overlays: dict[str, str], snippets: dict[str, str] | None = None):
        self.root = root
        self.overlays = overlays
        self.snippets = snippets or {}
        self.files = self._scan()
        self.dirs = self._directories()
        self.roots = self._roots()
        self._located: dict[tuple[str, str], ModuleLoc | None] = {}

    def _scan(self) -> frozenset[str]:
        found = set(self.overlays)
        for here, dirs, names in os.walk(self.root):
            rel_dir = os.path.relpath(here, self.root).replace(os.sep, "/")
            dirs[:] = sorted(d for d in dirs if d not in IGNORED_DIRS and not d.startswith("."))
            for name in names:
                if name.endswith(SOURCE_SUFFIXES):
                    rel = name if rel_dir == "." else posixpath.join(rel_dir, name)
                    if not is_ignored(rel):
                        found.add(rel)
        return frozenset(found)

    def _directories(self) -> frozenset[str]:
        found = {""}
        for rel in self.files:
            parent = posixpath.dirname(rel)
            while parent and parent not in found:
                found.add(parent)
                parent = posixpath.dirname(parent)
        return frozenset(found)

    def read(self, rel: str) -> str | None:
        """The text of a file: its overlay, else the disk; None when it cannot be read."""
        if rel in self.overlays:
            return self.overlays[rel]
        if rel in self.snippets:
            return self.snippets[rel]
        try:
            with open(os.path.join(self.root, rel), "rb") as handle:
                data = handle.read()
        except OSError:
            return None
        return decode(data)

    def exists(self, rel: str) -> bool:
        return rel in self.files

    def modules(self) -> frozenset[str]:
        """The module paths of the project's source files (stubs are not modules of their own)."""
        return frozenset(module_path(rel) for rel in self.files if not rel.endswith(".pyi"))

    def _roots(self) -> tuple[str, ...]:
        """The import roots: the project root, `src` where the layout says so, and declared package dirs."""
        roots = [""]
        extra = self._declared_roots()
        if "src" in self.dirs and (self.exists("pyproject.toml") or "src/__init__.py" not in self.files):
            if "src" not in extra and self._has_package("src"):
                extra.append("src")
        for root in extra:
            if root not in roots and root in self.dirs:
                roots.append(root)
        return tuple(roots)

    def _has_package(self, directory: str) -> bool:
        prefix = directory + "/"
        return any(rel.startswith(prefix) and rel.endswith(SOURCE_SUFFIXES) for rel in self.files)

    def _declared_roots(self) -> list[str]:
        text = self.overlays.get("pyproject.toml") or self._disk_text("pyproject.toml")
        if text is None:
            return []
        try:
            data = tomllib.loads(text)
        except tomllib.TOMLDecodeError:
            return []
        return declared_roots(data)

    def _disk_text(self, rel: str) -> str | None:
        path = os.path.join(self.root, rel)
        if not os.path.isfile(path):
            return None
        try:
            with open(path, "rb") as handle:
                return decode(handle.read())
        except OSError:
            return None

    def implicit_root(self, rel: str) -> str:
        """The directory above the topmost package a file lives in: where a script run beside it would import from."""
        directory = posixpath.dirname(rel)
        while directory and posixpath.join(directory, "__init__.py") in self.files:
            directory = posixpath.dirname(directory)
        return directory

    def find(self, dotted: str, from_rel: str | None = None) -> ModuleLoc | None:
        """The module a dotted name names, looked up the way the import system does: regular packages and modules
        first, in root order; a namespace package only when no root has a regular one."""
        implicit = self.implicit_root(from_rel) if from_rel is not None else ""
        key = (dotted, implicit)
        if key not in self._located:
            self._located[key] = self._find(dotted, implicit)
        return self._located[key]

    def _find(self, dotted: str, implicit: str) -> ModuleLoc | None:
        path = dotted.replace(".", "/")
        roots = [implicit] + [root for root in self.roots if root != implicit]
        namespace: ModuleLoc | None = None
        for root in roots:
            base = posixpath.join(root, path) if root else path
            found = self._regular(base)
            if found is not None:
                return found
            if namespace is None and base in self.dirs and self._has_package(base):
                namespace = ModuleLoc(module_path(base + "/__init__.py"), None, True)
        return namespace

    def _regular(self, base: str) -> ModuleLoc | None:
        for suffix in SOURCE_SUFFIXES:
            if base + suffix in self.files:
                return ModuleLoc(module_path(base + suffix), base + suffix, False)
        for suffix in SOURCE_SUFFIXES:
            init = posixpath.join(base, "__init__" + suffix)
            if init in self.files:
                return ModuleLoc(module_path(init), init, True)
        return None

    def find_relative(self, from_rel: str, level: int, dotted: str | None) -> ModuleLoc | None:
        """The module a relative import names: `level` dots from the package of the importing file."""
        package = posixpath.dirname(from_rel)
        for _ in range(level - 1):
            if package == "":
                return None
            package = posixpath.dirname(package)
        base = posixpath.join(package, dotted.replace(".", "/")) if dotted else package
        if base == "":
            return ModuleLoc(".", None, True) if self._has_package("") else None
        found = self._regular(base)
        if found is not None:
            return found
        if base in self.dirs and self._has_package(base):
            return ModuleLoc(module_path(base + "/__init__.py"), None, True)
        return None

    def relative_path(self, from_rel: str, level: int, dotted: str | None) -> str:
        """The module path a relative import names, whether or not it exists."""
        package = posixpath.dirname(from_rel)
        for _ in range(level - 1):
            package = posixpath.dirname(package)
        joined = posixpath.join(package, dotted.replace(".", "/")) if dotted else package
        return posixpath.normpath(joined) if joined else "."


def declared_roots(data: dict[str, Any]) -> list[str]:
    """The package directories a `pyproject.toml` declares statically, for setuptools and hatch."""
    found: list[str] = []
    tool = data.get("tool", {})
    setuptools = tool.get("setuptools", {})
    package_dir = setuptools.get("package-dir")
    if isinstance(package_dir, dict) and isinstance(package_dir.get(""), str):
        found.append(package_dir[""])
    where = (
        setuptools.get("packages", {}).get("find", {}).get("where")
        if isinstance(setuptools.get("packages"), dict)
        else None
    )
    if isinstance(where, list):
        found.extend(w for w in where if isinstance(w, str))
    hatch = tool.get("hatch", {}).get("build", {})
    targets = hatch.get("targets", {}) if isinstance(hatch, dict) else {}
    for section in [hatch] + [t for t in targets.values() if isinstance(t, dict)]:
        for package in section.get("packages", []) if isinstance(section.get("packages"), list) else []:
            if isinstance(package, str) and "/" in package:
                found.append(posixpath.dirname(package))
        sources = section.get("sources")
        if isinstance(sources, list):
            found.extend(s for s in sources if isinstance(s, str))
    return [posixpath.normpath(root) for root in found if root not in ("", ".")]


def decode(data: bytes) -> str:
    """Source bytes as text: a byte order mark and the coding cookie honoured, bad bytes replaced."""
    lines = iter(data.splitlines(keepends=True))
    try:
        encoding, _ = tokenize.detect_encoding(lambda: next(lines, b""))
    except (SyntaxError, LookupError):
        encoding = "utf-8"
    return data.decode(encoding, errors="replace").replace("\r\n", "\n").replace("\ufeff", "", 1)
