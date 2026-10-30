"""Paths, module names and the conventions that tell test files and ignored paths apart."""

import posixpath

SNIPPETS_DIR = ".lighthouse-snippets"
SOURCE_SUFFIXES = (".py", ".pyi")
IGNORED_DIRS = frozenset({"node_modules", "testdata", "__pycache__", "site-packages"})


def is_stub(rel: str) -> bool:
    return rel.endswith(".pyi")


def is_ignored(rel: str) -> bool:
    """Paths the provider answers with an empty fragment: dependencies, hidden directories, test data."""
    parts = rel.split("/")
    if SNIPPETS_DIR in parts:
        return any(part in IGNORED_DIRS for part in parts[:-1])
    return any(part in IGNORED_DIRS or part.startswith(".") for part in parts[:-1])


def module_path(rel: str) -> str:
    """The module a file declares: its path without the extension, a package's `__init__` as its directory."""
    for suffix in SOURCE_SUFFIXES:
        if rel.endswith(suffix):
            rel = rel[: -len(suffix)]
            break
    if posixpath.basename(rel) == "__init__":
        return posixpath.dirname(rel) or "."
    return rel


def is_test_file(rel: str) -> bool:
    """Whether a project-relative file is a test file by the conventions the manifest declares."""
    parts = rel.split("/")
    name = parts[-1]
    stem = name.rsplit(".", 1)[0]
    if stem == "conftest" or stem.startswith("test_") or stem.endswith("_test"):
        return True
    return "tests" in parts[:-1] or "test" in parts[:-1]


def tested_module(module: str) -> str | None:
    """The module a test file tests by naming alone: `a/test_b` and `a/b_test` name `a/b`."""
    directory, base = posixpath.split(module)
    stripped = base
    if base.startswith("test_"):
        stripped = base[len("test_") :]
    elif base.endswith("_test"):
        stripped = base[: -len("_test")]
    if stripped == base or stripped == "":
        return None
    return posixpath.join(directory, stripped) if directory else stripped


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


def id_part(name: str) -> str:
    """A part of a symbol id, with the characters the id format reserves escaped."""
    return name.replace("#", "%23").replace("::", ":%3A")
