"""The declarations of one source file: symbols, their owners and the statements that make them."""

import ast
from dataclasses import dataclass, field
from typing import Iterator

from lang_python.sdk.wire import SymbolKind


def _id_part(name: str) -> str:
    """A part of a symbol id, with the characters the id format reserves escaped."""
    return name.replace("#", "%23").replace("::", ":%3A")


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
    """A field a method sets (`self.name = ...`) rather than one the class body declares."""
    traits: frozenset[str] = frozenset()
    """What the class is: `enum`, `protocol`, `abstract`, `dataclass`, `testcase`, `test-class`."""


@dataclass(frozen=True)
class Origin:
    """The syntax a declaration comes from: the node that gives its span, the statement an extent runs over
    (None when it shares one with another name) and the function a summary measures."""

    node: ast.AST
    statement: ast.stmt | None
    body: Body | None = None


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
        self.following = _docstrings_after(tree)
        _Collector(self).run()

    def member(self, owner: Decl, name: str) -> Decl | None:
        """The member of a class by name; None when the class declares none of that name."""
        for decl in self.list:
            if decl.owner is owner and decl.name == name:
                return decl
        return None

    def of(self, node: ast.AST) -> Decl | None:
        """The declaration a node belongs to; None for a node no declaration owns."""
        return self.by_node.get(id(node))

    def add(self, kind: SymbolKind, name: str, owner: Decl | None, origin: Origin) -> Decl:
        """Adds a declaration, or merges the node into the one with the same id."""
        parts = [*_owner_names(owner), name]
        decl_id = "%s::%s#%s" % (self.module, "::".join(_id_part(p) for p in parts), kind)
        node = origin.node
        existing = self._ids.get(decl_id)
        if existing is not None:
            existing.nodes.append(node)
            existing.statements.append(origin.statement)
            if origin.body is not None and existing.body is None:
                existing.body = origin.body
                existing.node = node
            self.by_node[id(node)] = existing
            return existing
        decl = Decl(decl_id, kind, name, self.module, owner, node, [node], [origin.statement], origin.body)
        self.list.append(decl)
        self._ids[decl_id] = decl
        self.by_node[id(node)] = decl
        if owner is None:
            self.top.setdefault(name, decl)
        return decl


def _owner_names(owner: Decl | None) -> list[str]:
    return [] if owner is None else [*_owner_names(owner.owner), owner.name]


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
    """Whether a statement is a string expression, which PEP 257 reads as documentation."""
    return isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Constant) and isinstance(stmt.value.value, str)


def _decorator_names(node: ast.AST) -> list[str]:
    """The dotted names of the decorators of a def or class, calls looked through."""
    names = []
    for decorator in getattr(node, "decorator_list", []):
        target = decorator.func if isinstance(decorator, ast.Call) else decorator
        name = dotted(target)
        if name is not None:
            names.append(name)
    return names


def has_decorator(node: ast.AST, last: str) -> bool:
    """Whether a def or class has a decorator whose last dotted name is `last`."""
    return any(name.rsplit(".", 1)[-1] == last for name in _decorator_names(node))


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


def _is_constant_name(name: str) -> bool:
    letters = name.replace("_", "")
    return letters != "" and letters.upper() == letters and any(c.isalpha() for c in letters)


def is_dunder(name: str) -> bool:
    """Whether a name is a special name of the form `__name__`."""
    return len(name) > 4 and name.startswith("__") and name.endswith("__")


def _is_sunder(name: str) -> bool:
    return len(name) > 2 and name.startswith("_") and name.endswith("_") and not name.startswith("__")


@dataclass(frozen=True)
class _Assigned:
    """An assignment statement: the names it writes, its value and its annotation."""

    stmt: ast.stmt
    targets: list[ast.expr]
    value: ast.expr | None
    annotation: ast.expr | None

    @property
    def single(self) -> bool:
        """Whether one plain name is the whole target, so the statement is that name's declaration."""
        return len(self.targets) == 1 and isinstance(self.targets[0], ast.Name)

    @property
    def statement(self) -> ast.stmt | None:
        """The statement an extent runs over: the whole of it, unless it shares the statement with other names."""
        return self.stmt if self.single else None


class _Collector:
    """Collects the declarations of a module and of its classes into a [FileDecls]."""

    def __init__(self, decls: FileDecls):
        self.decls = decls

    def run(self) -> None:
        self._statements(self.decls.tree.body, None, None)
        self._module_body(self.decls.tree.body)

    def _statements(self, body: list[ast.stmt], owner: Decl | None, klass: ast.ClassDef | None) -> None:
        for stmt in walk_flat(body):
            self._statement(stmt, owner, klass)

    def _statement(self, stmt: ast.stmt, owner: Decl | None, klass: ast.ClassDef | None) -> None:
        if isinstance(stmt, FUNCTION_NODES):
            self._function(stmt, owner)
        elif isinstance(stmt, ast.ClassDef):
            self._klass(stmt, owner)
        elif isinstance(stmt, ast.Assign):
            self._assign(_Assigned(stmt, stmt.targets, stmt.value, None), owner)
        elif isinstance(stmt, ast.AnnAssign):
            self._assign(_Assigned(stmt, [stmt.target], stmt.value, stmt.annotation), owner)
        elif type(stmt).__name__ == "TypeAlias":
            name = stmt.name  # type: ignore[attr-defined]
            if isinstance(name, ast.Name):
                self.decls.add("type", name.id, owner, Origin(stmt, stmt))

    def _function(self, node: ast.FunctionDef | ast.AsyncFunctionDef, owner: Decl | None) -> None:
        body = None if has_decorator(node, "overload") else node
        self.decls.add(self._function_kind(node, owner), node.name, owner, Origin(node, node, body))
        if owner is not None and not (has_decorator(node, "staticmethod") or has_decorator(node, "classmethod")):
            self._instance_fields(node, owner)

    def _function_kind(self, node: ast.FunctionDef | ast.AsyncFunctionDef, owner: Decl | None) -> SymbolKind:
        if owner is None:
            return "test" if self.decls.test and node.name.startswith("test_") else "function"
        return "test" if self._is_test_method(owner, node) else "method"

    def _is_test_method(self, owner: Decl, node: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
        if "testcase" in owner.traits:
            return node.name.startswith("test")
        return "test-class" in owner.traits and node.name.startswith("test_")

    def _klass(self, node: ast.ClassDef, owner: Decl | None) -> None:
        traits = self._traits(node)
        kind: SymbolKind = "interface" if "protocol" in traits else "type"
        decl = self.decls.add(kind, node.name, owner, Origin(node, node))
        decl.traits = traits
        for stmt in walk_flat(node.body):
            self._statement(stmt, decl, node)

    def _traits(self, node: ast.ClassDef) -> frozenset[str]:
        found: set[str] = set()
        for base in node.bases:
            found |= self._base_traits(base)
        if any(k.arg == "metaclass" and (dotted(k.value) or "").endswith("ABCMeta") for k in node.keywords):
            found.add("abstract")
        if has_decorator(node, "dataclass"):
            found.add("dataclass")
        if self.decls.test and node.name.startswith("Test") and not self._defines_init(node):
            found.add("test-class")
        return frozenset(found)

    def _base_traits(self, base: ast.expr) -> set[str]:
        """What a base class says its subclass is: an enum, a protocol, an abstract class or a test case."""
        name = dotted(base.value if isinstance(base, ast.Subscript) else base) or ""
        last = name.rsplit(".", 1)[-1]
        found: set[str] = set()
        if last in ENUM_BASES or self._local_trait(last, "enum"):
            found.add("enum")
        if last == "Protocol":
            found.add("protocol")
        if last in ("ABC", "ABCMeta"):
            found.add("abstract")
        if last.endswith("TestCase") or self._local_trait(last, "testcase"):
            found.add("testcase")
        return found

    def _local_trait(self, name: str, trait: str) -> bool:
        decl = self.decls.top.get(name)
        return decl is not None and trait in decl.traits

    @staticmethod
    def _defines_init(node: ast.ClassDef) -> bool:
        return any(isinstance(s, FUNCTION_NODES) and s.name == "__init__" for s in node.body)

    def _assign(self, assigned: _Assigned, owner: Decl | None) -> None:
        names = [n for target in assigned.targets for n in bound_names(target)]
        for name in names:
            if is_dunder(name.id) or (owner is not None and "enum" in owner.traits and _is_sunder(name.id)):
                continue
            if owner is None:
                self._module_variable(assigned, name)
            else:
                self._member_variable(assigned, name, owner)

    def _module_variable(self, assigned: _Assigned, name: ast.Name) -> None:
        """A name a module binds: a lambda is a function, else a type alias, a constant or a variable."""
        if isinstance(assigned.value, ast.Lambda) and assigned.single and assigned.annotation is None:
            origin = Origin(assigned.stmt, assigned.statement, assigned.value)
            self._finish(self.decls.add("function", name.id, None, origin), assigned)
            return
        self._declare(_module_variable_kind(name.id, assigned.annotation), assigned, name, None)

    def _member_variable(self, assigned: _Assigned, name: ast.Name, owner: Decl) -> None:
        """A name a class body binds: an enum member or a field; a function body binds none."""
        if assigned.annotation is None and not isinstance(owner.node, ast.ClassDef):
            return
        kind: SymbolKind = "variant" if "enum" in owner.traits and assigned.annotation is None else "field"
        self._declare(kind, assigned, name, owner)

    def _declare(self, kind: SymbolKind, assigned: _Assigned, name: ast.Name, owner: Decl | None) -> None:
        node = assigned.stmt if assigned.single else name
        decl = self.decls.add(kind, name.id, owner, Origin(node, assigned.statement))
        if len(decl.nodes) == 1:
            self._finish(decl, assigned)

    def _finish(self, decl: Decl, assigned: _Assigned) -> None:
        decl.annotation = assigned.annotation
        decl.default = assigned.value is not None
        decl.doc_statement = self.decls.following.get(id(assigned.stmt))

    def _instance_fields(self, method: ast.FunctionDef | ast.AsyncFunctionDef, owner: Decl) -> None:
        """The fields a method sets: `self.name = value` among its statements, the first one declaring."""
        first = method.args.args[0].arg if method.args.args else None
        if first is None:
            return
        for stmt in walk_flat(method.body):
            targets, annotation = _assigned_targets(stmt)
            for target in targets:
                if not self._sets_new_field(target, first, owner):
                    continue
                decl = self.decls.add("field", target.attr, owner, Origin(target, stmt))  # type: ignore[attr-defined]
                decl.annotation = annotation
                decl.default = True
                decl.instance = True

    def _sets_new_field(self, target: ast.expr, receiver: str, owner: Decl) -> bool:
        """Whether a target is `receiver.name` for a name the class does not declare yet."""
        return (
            isinstance(target, ast.Attribute)
            and isinstance(target.value, ast.Name)
            and target.value.id == receiver
            and self.decls.member(owner, target.attr) is None
        )

    def _module_body(self, body: list[ast.stmt]) -> None:
        for stmt in body:
            if executable(stmt):
                self.decls.module_body = self.decls.add("function", MODULE_BODY, None, Origin(stmt, None))
                return


def _assigned_targets(stmt: ast.stmt) -> tuple[list[ast.expr], ast.expr | None]:
    """The targets an assignment writes, and the annotation of an annotated one."""
    if isinstance(stmt, ast.Assign):
        return list(stmt.targets), None
    if isinstance(stmt, ast.AnnAssign):
        return [stmt.target], stmt.annotation
    return [], None


def _module_variable_kind(name: str, annotation: ast.expr | None) -> SymbolKind:
    """What a name a module binds is: a type alias, a constant (`UPPER_CASE` or `Final`) or a variable."""
    if annotation is not None and (dotted(annotation) or "").rsplit(".", 1)[-1] == "TypeAlias":
        return "type"
    if _is_constant_name(name) or (annotation is not None and "Final" in ast.unparse(annotation)):
        return "const"
    return "var"


def _docstrings_after(tree: ast.Module) -> dict[int, ast.stmt]:
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


def _has_call(node: ast.AST | None) -> bool:
    return node is not None and any(isinstance(n, (ast.Call, ast.Await, ast.Yield)) for n in ast.walk(node))


def executable(stmt: ast.stmt) -> bool:
    """Whether a top-level statement runs something when the module loads that no declaration owns."""
    if isinstance(stmt, ast.Expr):
        return not is_docstring(stmt) and not (isinstance(stmt.value, ast.Constant) and stmt.value.value is Ellipsis)
    if isinstance(stmt, (ast.AugAssign, ast.Raise, ast.Assert, ast.Delete)):
        return True
    if isinstance(stmt, (ast.Assign, ast.AnnAssign)):
        return not _declares_only(stmt)
    headers, inner = _compound_parts(stmt)
    return any(_has_call(e) for e in headers) or any(executable(s) for s in inner)


def _compound_parts(stmt: ast.stmt) -> tuple[list[ast.expr], list[ast.stmt]]:
    """The expressions a compound statement evaluates before its body, and the statements of its bodies."""
    if isinstance(stmt, (ast.If, ast.While)):
        return [stmt.test], [*stmt.body, *stmt.orelse]
    if isinstance(stmt, (ast.For, ast.AsyncFor)):
        return [stmt.iter], [*stmt.body, *stmt.orelse]
    if isinstance(stmt, (ast.With, ast.AsyncWith)):
        return [item.context_expr for item in stmt.items], stmt.body
    if isinstance(stmt, ast.Try) or type(stmt).__name__ == "TryStar":
        inner = [*stmt.body, *stmt.orelse, *stmt.finalbody]  # type: ignore[attr-defined]
        return [], inner + [s for handler in stmt.handlers for s in handler.body]  # type: ignore[attr-defined]
    if isinstance(stmt, ast.Match):
        return [stmt.subject], [s for case in stmt.cases for s in case.body]
    return [], []


def _declares_only(stmt: ast.stmt) -> bool:
    """Whether an assignment binds names, which carry declarations, and does nothing else with its targets."""
    targets = stmt.targets if isinstance(stmt, ast.Assign) else [stmt.target]  # type: ignore[attr-defined]
    return any(bound_names(target) for target in targets)
