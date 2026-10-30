"""Modules: a module is a file, named by its path, and a test file names the module it tests."""

import posixpath

from lang_python.tests import is_test_file

SOURCE_SUFFIXES = (".py", ".pyi")


def module_path(rel: str) -> str:
    """The module a file declares: its path without the extension, a package's `__init__` as its directory."""
    for suffix in SOURCE_SUFFIXES:
        if rel.endswith(suffix):
            rel = rel[: -len(suffix)]
            break
    if posixpath.basename(rel) == "__init__":
        return posixpath.dirname(rel) or "."
    return rel


def _named_module(module: str) -> str | None:
    """The module a test module's name points at: `a/test_b` and `a/b_test` name `a/b`."""
    directory, base = posixpath.split(module)
    stripped = base
    if base.startswith("test_"):
        stripped = base[len("test_") :]
    elif base.endswith("_test"):
        stripped = base[: -len("_test")]
    if stripped == base or stripped == "":
        return None
    return posixpath.join(directory, stripped) if directory else stripped


def tested_module(module: str, known: frozenset[str]) -> str | None:
    """The module a test file tests by naming alone, when the project has it.

    `a/test_b` and `a/b_test` test `a/b`. A file in a `tests` directory mirrors the layout beside it: `a/tests/test_b`
    tests `a/b`, and failing that the one module `b` below `a`, at any depth; `tests/sub/test_b` tests `sub/b` likewise.
    """
    named = _named_module(module)
    if named is None:
        return None
    return named if named in known else _mirrored_module(named, known)


def _mirrored_module(named: str, known: frozenset[str]) -> str | None:
    """The module a name below a `tests` directory points at, looked for beside that directory."""
    parts = named.split("/")
    cut = max((i for i, part in enumerate(parts[:-1]) if part in ("tests", "test")), default=None)
    if cut is None:
        return None
    above, tail = "/".join(parts[:cut]), "/".join(parts[cut + 1 :])
    beside = posixpath.join(above, tail)
    if beside in known and not _is_test_module(beside):
        return beside
    found = [
        m
        for m in known
        if (m == tail or m.endswith("/" + tail)) and (not above or m.startswith(above + "/")) and not _is_test_module(m)
    ]
    return found[0] if len(found) == 1 else None


def _is_test_module(module: str) -> bool:
    """Whether a module, as a file or as a package, is a test file by the conventions the manifest declares."""
    return is_test_file(module + ".py") or is_test_file(module + "/__init__.py")


def directory_of(module: str) -> str:
    """The directory a test file tests as a whole: its own, or the one above `tests`."""
    parts = module.split("/")[:-1]
    while parts and parts[-1] in ("tests", "test"):
        parts.pop()
    return "/".join(parts) or "."


def is_private_module(module: str) -> bool:
    """Whether a path component of the module names an internal package or module by a leading underscore."""
    for part in module.split("/"):
        if part.startswith("_") and not (part.startswith("__") and part.endswith("__")):
            return True
    return False
