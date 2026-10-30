"""The declarations of one source file: symbols, their owners and the statements that make them."""

import ast
from dataclasses import dataclass, field
from typing import Iterator

from lang_python.paths import id_part
from lang_python.wire import SymbolKind

MODULE_BODY = "<module>"
ENUM_BASES = frozenset({"Enum", "IntEnum", "StrEnum", "Flag", "IntFlag", "ReprEnum"})
FUNCTION_NODES = (ast.FunctionDef, ast.AsyncFunctionDef)

Body = ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda


@dataclass(eq=False)
class Decl:
    """One declared name: all the declarations that merge into it (overloads, property accessors)."""

    id: str
    kind: SymbolKind
    name: str
    module: str
    owner: "Decl | None"
    node: ast.AST
    """The node whose range is the symbol's span: the def, the class, the assignment."""
    nodes: list[ast.AST] = field(default_factory=list)
    """Every declaration node of the name, in source order."""
    statements: list[ast.stmt | None] = field(default_factory=list)
    """Per node, the statement an extent runs over; None when it shares one with another name."""
    body: Body | None = None
    """The function the summary measures, when the name has a body."""
    doc_statement: ast.stmt | None = None
    """The string statement that documents a variable or field."""
    annotation: ast.expr | None = None
    default: bool = False
    instance: bool = False
    """A field a constructor sets (`self.name = ...`) rather than one the class body declares."""
    traits: frozenset[str] = frozenset()
    """What the class is: `enum`, `protocol`, `abstract`, `dataclass`, `testcase`, `test-class`."""


class FileDecls:
    """The declarations of one parsed file, in source order."""

    def __init__(self, rel: str, module: str, tree: ast.Module, text: str, test: bool):
        self.rel = rel
        self.module = module
        self.tree = tree
        self.text = text
        self.test = test
        self.list: list[Decl] = []
        self.top: dict[str, Decl] = {}
        self._ids: dict[str, Decl] = {}
        self.by_node: dict[int, Decl] = {}
        self.module_body: Decl | None = None
        self.following = docstrings_after(tree)
        Collector(self).run()

    def members(self, owner: Decl) -> list[Decl]:
        return [d for d in self.list if d.owner is owner]

    def member(self, owner: Decl, name: str) -> Decl | None:
        for decl in self.list:
            if decl.owner is owner and decl.name == name:
                return decl
        return None

    def of(self, node: ast.AST) -> Decl | None:
        return self.by_node.get(id(node))

    def add(
        self,
        kind: SymbolKind,
        name: str,
        owner: Decl | None,
        node: ast.AST,
        statement: ast.stmt | None,
        body: Body | None = None,
    ) -> Decl:
        """Adds a declaration, or merges the node into the one with the same id."""
        parts = [*owner_names(owner), name]
        decl_id = "%s::%s#%s" % (self.module, "::".join(id_part(p) for p in parts), kind)
        existing = self._ids.get(decl_id)
        if existing is not None:
            existing.nodes.append(node)
            existing.statements.append(statement)
            if body is not None and existing.body is None:
                existing.body = body
                existing.node = node
            self.by_node[id(node)] = existing
            return existing
        decl = Decl(decl_id, kind, name, self.module, owner, node, [node], [statement], body)
        self.list.append(decl)
        self._ids[decl_id] = decl
        self.by_node[id(node)] = decl
        if owner is None:
            self.top.setdefault(name, decl)
        return decl


def owner_names(owner: Decl | None) -> list[str]:
    return [] if owner is None else [*owner_names(owner.owner), owner.name]


def dotted(expr: ast.AST) -> str | None:
    """The dotted name an expression spells, `a.b.c`, or None for anything else."""
    parts: list[str] = []
    while isinstance(expr, ast.Attribute):
        parts.append(expr.attr)
        expr = expr.value
    if isinstance(expr, ast.Name):
        parts.append(expr.id)
        return ".".join(reversed(parts))
    return None


def is_docstring(stmt: ast.stmt) -> bool:
    return isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Constant) and isinstance(stmt.value.value, str)


def decorator_names(node: ast.AST) -> list[str]:
    """The dotted names of the decorators of a def or class, calls looked through."""
    names = []
    for decorator in getattr(node, "decorator_list", []):
        target = decorator.func if isinstance(decorator, ast.Call) else decorator
        name = dotted(target)
        if name is not None:
            names.append(name)
    return names


def has_decorator(node: ast.AST, last: str) -> bool:
    return any(name.rsplit(".", 1)[-1] == last for name in decorator_names(node))


def walk_flat(body: list[ast.stmt]) -> Iterator[ast.stmt]:
    """The statements of a body, through compound statements but not into defs and classes."""
    for stmt in body:
        yield stmt
        if isinstance(stmt, (ast.If, ast.For, ast.AsyncFor, ast.While)):
            yield from walk_flat(stmt.body)
            yield from walk_flat(stmt.orelse)
        elif isinstance(stmt, (ast.With, ast.AsyncWith)):
            yield from walk_flat(stmt.body)
        elif isinstance(stmt, ast.Try) or type(stmt).__name__ == "TryStar":
            yield from walk_flat(stmt.body)  # type: ignore[attr-defined]
            for handler in stmt.handlers:  # type: ignore[attr-defined]
                yield from walk_flat(handler.body)
            yield from walk_flat(stmt.orelse)  # type: ignore[attr-defined]
            yield from walk_flat(stmt.finalbody)  # type: ignore[attr-defined]
        elif isinstance(stmt, ast.Match):
            for case in stmt.cases:
                yield from walk_flat(case.body)


def bound_names(target: ast.expr) -> list[ast.Name]:
    """The names an assignment target binds."""
    if isinstance(target, ast.Name):
        return [target]
    if isinstance(target, (ast.Tuple, ast.List)):
        return [n for element in target.elts for n in bound_names(element)]
    if isinstance(target, ast.Starred):
        return bound_names(target.value)
    return []


def is_constant_name(name: str) -> bool:
    letters = name.replace("_", "")
    return letters != "" and letters.upper() == letters and any(c.isalpha() for c in letters)


def is_dunder(name: str) -> bool:
    return len(name) > 4 and name.startswith("__") and name.endswith("__")


def is_sunder(name: str) -> bool:
    return len(name) > 2 and name.startswith("_") and name.endswith("_") and not name.startswith("__")


class Collector:
    """Collects the declarations of a module and of its classes into a [FileDecls]."""

    def __init__(self, decls: FileDecls):
        self.decls = decls

    def run(self) -> None:
        self.statements(self.decls.tree.body, None, None)
        self.module_body(self.decls.tree.body)

    def statements(self, body: list[ast.stmt], owner: Decl | None, klass: ast.ClassDef | None) -> None:
        for stmt in walk_flat(body):
            self.statement(stmt, owner, klass)

    def statement(self, stmt: ast.stmt, owner: Decl | None, klass: ast.ClassDef | None) -> None:
        if isinstance(stmt, FUNCTION_NODES):
            self.function(stmt, owner)
        elif isinstance(stmt, ast.ClassDef):
            self.klass(stmt, owner)
        elif isinstance(stmt, ast.Assign):
            self.assign(stmt, stmt.targets, stmt.value, None, owner)
        elif isinstance(stmt, ast.AnnAssign):
            self.assign(stmt, [stmt.target], stmt.value, stmt.annotation, owner)
        elif type(stmt).__name__ == "TypeAlias":
            name = stmt.name  # type: ignore[attr-defined]
            if isinstance(name, ast.Name):
                self.decls.add("type", name.id, owner, stmt, stmt)

    def function(self, node: ast.FunctionDef | ast.AsyncFunctionDef, owner: Decl | None) -> None:
        kind: SymbolKind = "function" if owner is None else "method"
        if owner is None and self.decls.test and node.name.startswith("test_"):
            kind = "test"
        elif owner is not None and self.is_test_method(owner, node):
            kind = "test"
        overload = has_decorator(node, "overload")
        body = None if overload else node
        decl = self.decls.add(kind, node.name, owner, node, node, body)
        if kind == "method" and node.name == "__init__" and owner is not None and decl.nodes[0] is node:
            self.instance_fields(node, owner)

    def is_test_method(self, owner: Decl, node: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
        if "testcase" in owner.traits:
            return node.name.startswith("test")
        return "test-class" in owner.traits and node.name.startswith("test_")

    def klass(self, node: ast.ClassDef, owner: Decl | None) -> None:
        traits = self.traits(node, owner)
        kind: SymbolKind = "interface" if "protocol" in traits else "type"
        decl = self.decls.add(kind, node.name, owner, node, node)
        decl.traits = traits
        for stmt in walk_flat(node.body):
            self.statement(stmt, decl, node)

    def traits(self, node: ast.ClassDef, owner: Decl | None) -> frozenset[str]:
        found: set[str] = set()
        for base in node.bases:
            name = dotted(base.value if isinstance(base, ast.Subscript) else base) or ""
            last = name.rsplit(".", 1)[-1]
            if last in ENUM_BASES or self.local_trait(last, "enum"):
                found.add("enum")
            if last == "Protocol":
                found.add("protocol")
            if last in ("ABC", "ABCMeta"):
                found.add("abstract")
            if last.endswith("TestCase") or self.local_trait(last, "testcase"):
                found.add("testcase")
        for keyword in node.keywords:
            if keyword.arg == "metaclass" and (dotted(keyword.value) or "").endswith("ABCMeta"):
                found.add("abstract")
        if has_decorator(node, "dataclass"):
            found.add("dataclass")
        if self.decls.test and node.name.startswith("Test") and not self.defines_init(node):
            found.add("test-class")
        return frozenset(found)

    def local_trait(self, name: str, trait: str) -> bool:
        decl = self.decls.top.get(name)
        return decl is not None and trait in decl.traits

    @staticmethod
    def defines_init(node: ast.ClassDef) -> bool:
        return any(isinstance(s, FUNCTION_NODES) and s.name == "__init__" for s in node.body)

    def assign(
        self,
        stmt: ast.stmt,
        targets: list[ast.expr],
        value: ast.expr | None,
        annotation: ast.expr | None,
        owner: Decl | None,
    ) -> None:
        names = [n for target in targets for n in bound_names(target)]
        single = len(names) == 1 and len(targets) == 1 and isinstance(targets[0], ast.Name)
        for name in names:
            if is_dunder(name.id) or (owner is not None and "enum" in owner.traits and is_sunder(name.id)):
                continue
            self.variable(stmt, name, value, annotation, owner, single)

    def variable(
        self,
        stmt: ast.stmt,
        name: ast.Name,
        value: ast.expr | None,
        annotation: ast.expr | None,
        owner: Decl | None,
        single: bool,
    ) -> None:
        statement = stmt if single else None
        if owner is not None:
            kind: SymbolKind = "variant" if "enum" in owner.traits and annotation is None else "field"
            if annotation is None and not isinstance(owner.node, ast.ClassDef):
                return
        elif isinstance(value, ast.Lambda) and single and annotation is None:
            decl = self.decls.add("function", name.id, owner, stmt, statement, value)
            self.finish(decl, stmt, annotation, value, owner)
            return
        elif annotation is not None and (dotted(annotation) or "").rsplit(".", 1)[-1] == "TypeAlias":
            kind = "type"
        elif is_constant_name(name.id) or (annotation is not None and "Final" in (ast.unparse(annotation))):
            kind = "const"
        else:
            kind = "var"
        decl = self.decls.add(kind, name.id, owner, stmt if single else name, statement)
        if len(decl.nodes) == 1:
            self.finish(decl, stmt, annotation, value, owner)

    def finish(self, decl: Decl, stmt: ast.stmt, annotation: ast.expr | None, value: ast.expr | None, owner) -> None:
        decl.annotation = annotation
        decl.default = value is not None
        decl.doc_statement = self.decls.following.get(id(stmt))

    def instance_fields(self, init: ast.FunctionDef | ast.AsyncFunctionDef, owner: Decl) -> None:
        """The fields a constructor sets: `self.name = value` among its statements."""
        first = init.args.args[0].arg if init.args.args else None
        if first is None:
            return
        for stmt in walk_flat(init.body):
            targets: list[ast.expr] = []
            annotation = None
            if isinstance(stmt, ast.Assign):
                targets = list(stmt.targets)
            elif isinstance(stmt, ast.AnnAssign):
                targets, annotation = [stmt.target], stmt.annotation
            for target in targets:
                if not (isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name)):
                    continue
                if target.value.id != first or self.decls.member(owner, target.attr) is not None:
                    continue
                decl = self.decls.add("field", target.attr, owner, target, stmt)
                decl.annotation = annotation
                decl.default = True
                decl.instance = True

    def module_body(self, body: list[ast.stmt]) -> None:
        for stmt in body:
            if executable(stmt, self.decls):
                self.decls.module_body = self.decls.add("function", MODULE_BODY, None, stmt, None)
                return


def docstrings_after(tree: ast.Module) -> dict[int, ast.stmt]:
    """For each statement, the string statement right after it in its body: PEP 257's attribute docstring."""
    found: dict[int, ast.stmt] = {}
    for container in ast.walk(tree):
        for name in ("body", "orelse", "finalbody"):
            body = getattr(container, name, None)
            if not isinstance(body, list):
                continue
            for before, after in zip(body, body[1:]):
                if isinstance(before, ast.stmt) and isinstance(after, ast.stmt) and is_docstring(after):
                    found[id(before)] = after
    return found


def has_call(node: ast.AST | None) -> bool:
    return node is not None and any(isinstance(n, (ast.Call, ast.Await, ast.Yield)) for n in ast.walk(node))


def executable(stmt: ast.stmt, decls: FileDecls) -> bool:
    """Whether a top-level statement runs something when the module loads that no declaration owns."""
    if isinstance(stmt, ast.Expr):
        return not is_docstring(stmt) and not (isinstance(stmt.value, ast.Constant) and stmt.value.value is Ellipsis)
    if isinstance(stmt, (ast.AugAssign, ast.Raise, ast.Assert, ast.Delete)):
        return True
    if isinstance(stmt, (ast.Assign, ast.AnnAssign)):
        return not declares_only(stmt)
    if isinstance(stmt, (ast.If, ast.While)):
        return has_call(stmt.test) or any(executable(s, decls) for s in [*stmt.body, *stmt.orelse])
    if isinstance(stmt, (ast.For, ast.AsyncFor)):
        return has_call(stmt.iter) or any(executable(s, decls) for s in [*stmt.body, *stmt.orelse])
    if isinstance(stmt, (ast.With, ast.AsyncWith)):
        return any(has_call(item.context_expr) for item in stmt.items) or any(executable(s, decls) for s in stmt.body)
    if isinstance(stmt, ast.Try) or type(stmt).__name__ == "TryStar":
        inner = [*stmt.body, *stmt.orelse, *stmt.finalbody]  # type: ignore[attr-defined]
        inner += [s for handler in stmt.handlers for s in handler.body]  # type: ignore[attr-defined]
        return any(executable(s, decls) for s in inner)
    if isinstance(stmt, ast.Match):
        return has_call(stmt.subject) or any(executable(s, decls) for case in stmt.cases for s in case.body)
    return False


def declares_only(stmt: ast.stmt) -> bool:
    """Whether an assignment binds names, which carry declarations, and does nothing else with its targets."""
    targets = stmt.targets if isinstance(stmt, ast.Assign) else [stmt.target]  # type: ignore[attr-defined]
    return any(bound_names(target) for target in targets)
