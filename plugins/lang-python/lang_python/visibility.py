"""How far the names a file declares reach: PEP 8's leading underscore, `__all__` and re-exports."""

import posixpath

from lang_python.symbols import Decl, is_dunder
from lang_python.project import Model
from lang_python.modules import is_private_module, module_path
from lang_python.tests import is_test_file
from lang_python.project import SNIPPETS_DIR, DeclVal, Resolver, resolver_of, static_all
from lang_python.sdk.wire import Visibility

RANK: dict[Visibility, int] = {"private": 0, "internal": 1, "public": 2}


def _least(a: Visibility, b: Visibility) -> Visibility:
    return a if RANK[a] <= RANK[b] else b


def _is_underscored(name: str) -> bool:
    """Whether a name is private by convention: one or two leading underscores, but not a dunder."""
    return name.startswith("_") and not is_dunder(name)


class Reach:
    """Decides the visibility of declarations."""

    def __init__(self, model: Model):
        self.model = model
        self._exported: dict[str, frozenset[str]] = {}
        self._reexported: dict[str, frozenset[str]] = {}

    def visibility(self, decl: Decl, rel: str) -> Visibility:
        """How far the name a declaration binds reaches: public, internal to the project, or private."""
        if is_test_file(rel):
            return "private"
        owner = decl.owner
        if owner is None:
            return self._top_level(decl, rel)
        outer = self.visibility(owner, rel)
        mine: Visibility = "private" if _is_underscored(decl.name) else "public"
        # A public member of a class its module keeps to itself is still reached through the instances it hands out.
        if mine == "public" and outer == "private":
            return "internal"
        return _least(outer, mine)

    def _top_level(self, decl: Decl, rel: str) -> Visibility:
        if decl.name == "<module>":
            return "private"
        names = self._all_names(rel)
        lifted = decl.name in self._reexports(rel)
        if names is not None:
            public = decl.name in names or lifted
        elif SNIPPETS_DIR in rel.split("/"):
            # A fence is an example nothing imports: it exports only what its own `__all__` lists.
            public = False
        else:
            public = not _is_underscored(decl.name) or lifted
        if not public:
            return "private"
        return "internal" if is_private_module(decl.module) else "public"

    def _all_names(self, rel: str) -> frozenset[str] | None:
        """The names the module's `__all__` lists, when it states them statically."""
        if rel not in self._exported:
            parsed = self.model.file(rel)
            listed = static_all(parsed.tree) if parsed is not None and parsed.tree is not None else None
            self._exported[rel] = None if listed is None else frozenset(c.value for c in listed)  # type: ignore[assignment]
        return self._exported[rel]  # type: ignore[return-value]

    def _reexports(self, rel: str) -> frozenset[str]:
        """The names of a module that a package above it re-exports: imported in its `__init__.py` and
        listed in that file's `__all__`."""
        if rel not in self._reexported:
            self._reexported[rel] = self._find_reexports(rel)
        return self._reexported[rel]

    def _find_reexports(self, rel: str) -> frozenset[str]:
        found: set[str] = set()
        directory = posixpath.dirname(rel)
        while True:
            init = posixpath.join(directory, "__init__.py")
            if init != rel and init in self.model.project.files:
                found |= self._exports_through(init, rel)
            if directory == "":
                break
            directory = posixpath.dirname(directory)
        return frozenset(found)

    def _exports_through(self, init: str, rel: str) -> set[str]:
        parsed = self.model.file(init)
        listed = self._all_names(init)
        if parsed is None or parsed.decls is None or not listed:
            return set()
        resolver: Resolver = resolver_of(self.model, parsed)
        names = set()
        for name in listed:
            binding = resolver.module_imports().get(name)
            value = resolver.bound(binding) if binding is not None else None
            if isinstance(value, DeclVal) and self._lives_in(value.decl, rel):
                names.add(value.decl.name)
        return names

    @staticmethod
    def _lives_in(decl: Decl, rel: str) -> bool:
        return decl.owner is None and decl.module == module_path(rel)
