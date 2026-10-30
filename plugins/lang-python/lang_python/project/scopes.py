"""Scopes: what `symtable` says of where each name of a file is bound."""

import ast
import symtable
from collections import defaultdict, deque
from dataclasses import dataclass, field

COMPREHENSIONS = (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)


@dataclass(eq=False)
class Scope:
    """One scope of a file: a module, a class, a function, a lambda or a comprehension."""

    kind: str
    """`module`, `class` or `function` (lambdas and comprehensions are functions)."""
    table: symtable.SymbolTable | None
    parent: "Scope | None"
    node: ast.AST | None = None
    cache: dict = field(default_factory=dict)

    def symbol(self, name: str) -> symtable.Symbol | None:
        """The `symtable` symbol of a name in this scope; None when the scope has no table or the name is unknown."""
        if self.table is None:
            return None
        try:
            return self.table.lookup(name)
        except KeyError:
            return None


class ScopeTree:
    """The tables of a file, handed out to the scopes of its AST in source order."""

    def __init__(self, text: str, rel: str):
        self.queues: dict[tuple[str, int], deque[symtable.SymbolTable]] = defaultdict(deque)
        self.root: symtable.SymbolTable | None
        try:
            self.root = symtable.symtable(text, rel, "exec")
        except (SyntaxError, ValueError):
            self.root = None
        else:
            self._collect(self.root)

    def _collect(self, table: symtable.SymbolTable) -> None:
        for child in table.get_children():
            if child.get_type() in ("function", "class"):
                self.queues[(child.get_name(), child.get_lineno())].append(child)
            self._collect(child)

    def module(self, node: ast.Module) -> Scope:
        """The module scope of the tree."""
        return Scope("module", self.root, None, node)

    def enter(self, node: ast.AST, parent: Scope) -> Scope:
        """The scope a def, class, lambda or comprehension opens inside a scope."""
        kind = "class" if isinstance(node, ast.ClassDef) else "function"
        key = (_scope_name(node), getattr(node, "lineno", 0))
        queue = self.queues.get(key)
        table = queue.popleft() if queue else None
        return Scope(kind, table, parent, node)


def _scope_name(node: ast.AST) -> str:
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
        return node.name
    if isinstance(node, ast.Lambda):
        return "lambda"
    return {
        ast.ListComp: "listcomp",
        ast.SetComp: "setcomp",
        ast.DictComp: "dictcomp",
        ast.GeneratorExp: "genexpr",
    }[type(node)]


def binding_scope(scope: Scope, name: str) -> Scope | None:
    """The scope that binds a name used in a scope: None for a module-level or builtin name.

    Class scopes are skipped by the scopes nested in them, as Python does."""
    symbol = scope.symbol(name)
    if symbol is None or symbol.is_declared_global() or scope.kind == "module" or symbol.is_global():
        return None
    if symbol.is_local():
        return scope
    return _enclosing_binder(scope, name)


def _enclosing_binder(scope: Scope, name: str) -> Scope | None:
    """The nearest enclosing function scope that binds a free name."""
    outer = scope.parent
    while outer is not None:
        if outer.kind == "function":
            found = outer.symbol(name)
            if found is not None and found.is_local() and not found.is_declared_global():
                return outer
        outer = outer.parent
    return None
