"""The project: its files and import roots, the parsed files and scopes, and names resolved to declarations."""

from lang_python.project.files import IGNORED_DIRS, SNIPPETS_DIR, STDLIB, ModuleLoc, Project, decode, declared_roots
from lang_python.project.files import is_ignored, is_stub
from lang_python.project.model import Model, ParsedFile
from lang_python.project.resolve import (
    DeclVal,
    InstanceVal,
    ModuleVal,
    Resolver,
    constants,
    has_name,
    resolver_of,
    static_all,
    unwrap_annotation,
)
from lang_python.project.scopes import COMPREHENSIONS, Scope, ScopeTree, binding_scope

__all__ = [
    "COMPREHENSIONS",
    "IGNORED_DIRS",
    "SNIPPETS_DIR",
    "STDLIB",
    "DeclVal",
    "InstanceVal",
    "Model",
    "ModuleLoc",
    "ModuleVal",
    "ParsedFile",
    "Project",
    "Resolver",
    "Scope",
    "ScopeTree",
    "binding_scope",
    "constants",
    "decode",
    "declared_roots",
    "has_name",
    "is_ignored",
    "is_stub",
    "resolver_of",
    "static_all",
    "unwrap_annotation",
]
