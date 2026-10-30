"""The edges a file states: what each declaration calls, references, imports and implements."""

import ast
import builtins
from typing import Protocol

from lang_python.decls import FUNCTION_NODES, Decl, bound_names
from lang_python.model import Model, ParsedFile
from lang_python.positions import Lines, span, span_of
from lang_python.project import STDLIB, ModuleLoc
from lang_python.resolve import (
    DeclVal,
    ModuleVal,
    Resolver,
    constants,
    resolver_of,
    static_all,
)
from lang_python.scopes import COMPREHENSIONS, Scope
from lang_python.wire import EdgeKind, Node, Resolution, Span

MAX_CANDIDATES = 8
CALLABLE = ("function", "method", "test")
TYPES = ("type", "interface")
GUESS_CALLS = frozenset({"getattr", "hasattr", "setattr", "delattr"})


class EdgeSink(Protocol):
    def edge(self, kind: EdgeKind, source: Node, to: str, site: Span | None, resolution: Resolution) -> None: ...


class Uses:
    """Walks a file once and states what each declaration calls, references, imports and implements."""

    def __init__(self, model: Model, parsed: ParsedFile, sink: EdgeSink, fence: bool = False):
        self.model = model
        self.parsed = parsed
        self.decls = parsed.decls
        self.sink = sink
        self.fence = fence
        self.lines: Lines = parsed.lines
        self.resolver: Resolver = resolver_of(model, parsed)
        self.module: Node = {"module": self.decls.module}
        self.scopes: dict[int, Scope] = {}
        self.unresolved_imports = 0
        self.guessed = False

    def run(self) -> None:
        tree = self.parsed.tree
        scope = self.parsed.scopes().module(tree)
        body = self.decls.module_body
        owner: Node = {"symbol": body.id} if body is not None else self.module
        self.exports(tree)
        for stmt in tree.body:
            self.visit(stmt, scope, owner)

    # statements

    def visit(self, node: ast.AST, scope: Scope, owner: Node) -> None:
        if isinstance(node, FUNCTION_NODES):
            self.function(node, scope, owner)
        elif isinstance(node, ast.ClassDef):
            self.klass(node, scope, owner)
        elif isinstance(node, ast.Import):
            self.import_statement(node)
        elif isinstance(node, ast.ImportFrom):
            self.import_from(node)
        elif isinstance(node, (ast.Assign, ast.AnnAssign)):
            self.assignment(node, scope, owner)
        elif isinstance(node, ast.Lambda):
            self.lambda_(node, scope, owner)
        elif isinstance(node, COMPREHENSIONS):
            self.comprehension(node, scope, owner)
        elif isinstance(node, ast.Call):
            self.call(node, scope, owner)
        elif isinstance(node, ast.Attribute):
            self.attribute(node, scope, owner, call=False)
        elif isinstance(node, ast.Name):
            if isinstance(node.ctx, ast.Load):
                self.use_name(node, scope, owner, call=False)
        else:
            self.children(node, scope, owner)

    def children(self, node: ast.AST, scope: Scope, owner: Node) -> None:
        for child in ast.iter_child_nodes(node):
            self.visit(child, scope, owner)

    def declared(self, node: ast.AST, owner: Node) -> Node:
        decl = self.decls.of(node)
        return owner if decl is None else {"symbol": decl.id}

    def function(self, node: ast.FunctionDef | ast.AsyncFunctionDef, scope: Scope, owner: Node) -> None:
        here = self.declared(node, owner)
        for decorator in node.decorator_list:
            self.decorator(decorator, scope, here)
        args = node.args
        for default in [*args.defaults, *(d for d in args.kw_defaults if d is not None)]:
            self.visit(default, scope, here)
        for arg in [*args.posonlyargs, *args.args, *args.kwonlyargs, args.vararg, args.kwarg]:
            if arg is not None and arg.annotation is not None:
                self.annotation(arg.annotation, scope, here)
        if node.returns is not None:
            self.annotation(node.returns, scope, here)
        inner = self.parsed.scopes().enter(node, scope)
        self.scopes[id(node)] = inner
        for stmt in node.body:
            self.visit(stmt, inner, here)

    def lambda_(self, node: ast.Lambda, scope: Scope, owner: Node) -> None:
        args = node.args
        for default in [*args.defaults, *(d for d in args.kw_defaults if d is not None)]:
            self.visit(default, scope, owner)
        inner = self.parsed.scopes().enter(node, scope)
        self.scopes[id(node)] = inner
        self.visit(node.body, inner, owner)

    def comprehension(self, node: ast.AST, scope: Scope, owner: Node) -> None:
        generators = node.generators  # type: ignore[attr-defined]
        self.visit(generators[0].iter, scope, owner)
        inner = self.parsed.scopes().enter(node, scope)
        for index, generator in enumerate(generators):
            if index > 0:
                self.visit(generator.iter, inner, owner)
            self.visit(generator.target, inner, owner)
            for condition in generator.ifs:
                self.visit(condition, inner, owner)
        if isinstance(node, ast.DictComp):
            self.visit(node.key, inner, owner)
            self.visit(node.value, inner, owner)
        else:
            self.visit(node.elt, inner, owner)  # type: ignore[attr-defined]

    def klass(self, node: ast.ClassDef, scope: Scope, owner: Node) -> None:
        here = self.declared(node, owner)
        decl = self.decls.of(node)
        for decorator in node.decorator_list:
            self.decorator(decorator, scope, here)
        for base in node.bases:
            self.heritage(base, scope, here, decl)
        for keyword in node.keywords:
            self.visit(keyword.value, scope, here)
        inner = self.parsed.scopes().enter(node, scope)
        for stmt in node.body:
            self.visit(stmt, inner, here)

    def assignment(self, node: ast.Assign | ast.AnnAssign, scope: Scope, owner: Node) -> None:
        targets = node.targets if isinstance(node, ast.Assign) else [node.target]
        here = self.declared(node, owner)
        if here is owner:
            for target in targets:
                names = bound_names(target)
                decl = self.decls.of(names[0]) if names else None
                if decl is not None:
                    here = {"symbol": decl.id}
                    break
        if isinstance(node, ast.AnnAssign):
            self.annotation(node.annotation, scope, here)
        if node.value is not None:
            self.visit(node.value, scope, here)
        for target in targets:
            if not isinstance(target, ast.Name):
                self.visit_target(target, scope, here)

    def visit_target(self, target: ast.expr, scope: Scope, owner: Node) -> None:
        """The parts of an assignment target that are uses: `a.b`, `a[i]` read `a` and `i`."""
        if isinstance(target, (ast.Tuple, ast.List)):
            for element in target.elts:
                self.visit_target(element, scope, owner)
        elif isinstance(target, ast.Attribute):
            self.attribute(target, scope, owner, call=False)
        elif isinstance(target, ast.Subscript):
            self.visit(target.value, scope, owner)
            self.visit(target.slice, scope, owner)
        elif isinstance(target, ast.Starred):
            self.visit_target(target.value, scope, owner)

    def decorator(self, node: ast.expr, scope: Scope, owner: Node) -> None:
        if isinstance(node, ast.Call):
            self.call(node, scope, owner)
        elif isinstance(node, ast.Attribute):
            self.attribute(node, scope, owner, call=True)
        elif isinstance(node, ast.Name):
            self.use_name(node, scope, owner, call=True)
        else:
            self.visit(node, scope, owner)

    def heritage(self, base: ast.expr, scope: Scope, owner: Node, decl: Decl | None) -> None:
        """A base class: `implements` when it is a project interface or abstract class, else a reference."""
        inner = base.value if isinstance(base, ast.Subscript) else base
        value = self.resolver.expr_value(inner, scope) if isinstance(inner, (ast.Name, ast.Attribute)) else None
        if isinstance(value, DeclVal) and value.decl.kind in TYPES:
            self.walk_names(inner, scope, owner)
            kind: EdgeKind = "implements" if self.is_contract(value.decl) else "references"
            self.sink.edge(kind, owner, value.decl.id, self.name_span(inner), "semantic")
            if isinstance(base, ast.Subscript):
                self.visit(base.slice, scope, owner)
            return
        self.visit(base, scope, owner)

    def is_contract(self, cls: Decl) -> bool:
        return cls.kind == "interface" or "abstract" in cls.traits

    def walk_names(self, expr: ast.expr, scope: Scope, owner: Node) -> None:
        """The qualifier of a dotted name is a use of its first name; the last name is stated by the caller."""
        if isinstance(expr, ast.Attribute):
            self.visit(expr.value, scope, owner)

    # imports

    def import_statement(self, node: ast.Import) -> None:
        site = span_of(node)
        for alias in node.names:
            self.import_edge(0, alias.name, site)

    def import_from(self, node: ast.ImportFrom) -> None:
        site = span_of(node)
        loc = self.import_edge(node.level, node.module, site)
        for alias in node.names:
            if alias.name == "*" or loc is None:
                continue
            value = self.resolver.module_attribute(ModuleVal(node.module or "", loc), alias.name)
            if isinstance(value, ModuleVal) and value.loc is not None:
                self.sink.edge("imports", self.module, value.loc.module, site, "semantic")
            elif isinstance(value, DeclVal) and self.parsed.rel.endswith("__init__.py"):
                self.sink.edge("references", self.module, value.decl.id, site, "semantic")

    def import_edge(self, level: int, module: str | None, site: Span) -> ModuleLoc | None:
        """An `imports` edge to the module an import names; the project module it resolves to, if any."""
        rel = self.parsed.rel
        if level == 0 and module is not None:
            loc = self.model.project.find(module, rel)
            if loc is not None:
                self.sink.edge("imports", self.module, loc.module, site, "semantic")
                return loc
            resolution: Resolution = "semantic" if module.split(".")[0] in STDLIB else "syntactic"
            self.sink.edge("imports", self.module, module, site, resolution)
            return None
        loc = self.model.project.find_relative(rel, level, module)
        if loc is not None:
            self.sink.edge("imports", self.module, loc.module, site, "semantic")
            return loc
        self.unresolved_imports += 1
        self.sink.edge("imports", self.module, self.model.project.relative_path(rel, level, module), site, "syntactic")
        return None

    def exports(self, tree: ast.Module) -> None:
        """The names of `__all__` are uses of what they name, by the module."""
        for constant in static_all(tree) or []:
            value = self.resolver.module_value(constant.value)
            if isinstance(value, DeclVal):
                self.sink.edge("references", self.module, value.decl.id, span_of(constant), "semantic")

    # names

    def call(self, node: ast.Call, scope: Scope, owner: Node) -> None:
        func = node.func
        if isinstance(func, ast.Name):
            self.use_name(func, scope, owner, call=True)
            self.guess_member(node, func.id, scope, owner)
        elif isinstance(func, ast.Attribute):
            self.attribute(func, scope, owner, call=True)
        else:
            self.visit(func, scope, owner)
        for argument in node.args:
            self.visit(argument, scope, owner)
        for keyword in node.keywords:
            self.visit(keyword.value, scope, owner)

    def guess_member(self, node: ast.Call, name: str, scope: Scope, owner: Node) -> None:
        """`getattr(x, "name")` uses a member named `name` of something unknown."""
        if name not in GUESS_CALLS or len(node.args) < 2 or self.is_bound(name, scope):
            return
        for literal in constants(node.args[1]):
            self.by_name(literal.value, span_of(literal), owner, call=False, members=True)

    def is_bound(self, name: str, scope: Scope) -> bool:
        return self.resolver.lookup(name, scope) is not None or name in self.decls.top

    def use_name(self, node: ast.Name, scope: Scope, owner: Node, call: bool) -> None:
        value = self.resolver.lookup(node.id, scope)
        if isinstance(value, DeclVal):
            self.to_decl(value.decl, span_of(node), owner, call, "semantic")
        elif value is None and self.fence and self.undefined(node.id, scope):
            self.by_name(node.id, span_of(node), owner, call, members=False)

    def undefined(self, name: str, scope: Scope) -> bool:
        """Whether a bare name is bound by nothing in the file and is no builtin: in a fence, the prose's."""
        if hasattr(builtins, name):
            return False
        symbol = scope.symbol(name)
        return symbol is None or not (symbol.is_local() or symbol.is_parameter() or symbol.is_imported())

    def attribute(self, node: ast.Attribute, scope: Scope, owner: Node, call: bool) -> None:
        self.visit(node.value, scope, owner)
        site = self.attr_span(node)
        if self.is_super(node.value, scope):
            self.super_member(node, scope, owner, call, site)
            return
        base = self.resolver.expr_value(node.value, scope)
        if isinstance(base, ModuleVal) and base.loc is None:
            return
        found = None if base is None else self.resolver.attribute(base, node.attr)
        if isinstance(found, DeclVal):
            if found.decl.node is node:
                return
            self.to_decl(found.decl, site, owner, call, "semantic")
        elif isinstance(found, ModuleVal):
            return
        elif isinstance(base, ModuleVal):
            return
        else:
            self.by_name(node.attr, site, owner, call, members=True)

    def is_super(self, expr: ast.expr, scope: Scope) -> bool:
        return (
            isinstance(expr, ast.Call)
            and isinstance(expr.func, ast.Name)
            and expr.func.id == "super"
            and self.resolver.lookup("super", scope) is None
        )

    def super_member(self, node: ast.Attribute, scope: Scope, owner: Node, call: bool, site: Span) -> None:
        """`super().name` is the member of the nearest base class that has it."""
        cls = self.enclosing_class(scope)
        if cls is None:
            return
        for base in self.resolver.ancestors(cls):
            found = self.resolver.member(base, node.attr)
            if found is not None:
                self.to_decl(found, site, owner, call, "semantic")
                return

    def enclosing_class(self, scope: Scope) -> Decl | None:
        """The project class the code of a scope belongs to."""
        current: Scope | None = scope
        while current is not None:
            parent = current.parent
            if current.kind == "function" and parent is not None and parent.kind == "class" and parent.node:
                return self.decls.of(parent.node)
            current = parent
        return None

    def by_name(self, name: str, site: Span, owner: Node, call: bool, members: bool) -> None:
        """A use of a name the file cannot resolve: each project declaration that bears it, when few do."""
        candidates = [d for d in self.model.named(name) if (d.owner is not None) == members and d.name != "<module>"]
        if members:
            candidates = [d for d in candidates if d.kind != "test"]
        if not candidates or len(candidates) > MAX_CANDIDATES:
            return
        self.guessed = True
        for decl in candidates:
            if decl.id != owner.get("symbol"):
                self.to_decl(decl, site, owner, call, "syntactic")

    def to_decl(self, decl: Decl, site: Span, owner: Node, call: bool, resolution: Resolution) -> None:
        source = owner.get("symbol")
        if call and decl.kind in TYPES:
            constructor = self.resolver.member(decl, "__init__")
            if constructor is not None:
                self.sink.edge("calls", owner, constructor.id, site, resolution)
                return
        kind: EdgeKind = "calls" if call and decl.kind in CALLABLE else "references"
        if kind == "references" and source == decl.id:
            return
        self.sink.edge(kind, owner, decl.id, site, resolution)

    # annotations

    def annotation(self, expr: ast.expr, scope: Scope, owner: Node) -> None:
        """A type annotation: its names are uses; a quoted one is read with positions mapped into the quotes."""
        if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
            inner = self.quoted(expr)
            if inner is not None:
                self.annotation(inner, scope, owner)
            return
        if isinstance(expr, ast.Subscript):
            self.annotation(expr.value, scope, owner)
            slice_ = expr.slice
            for part in slice_.elts if isinstance(slice_, ast.Tuple) else [slice_]:
                self.annotation(part, scope, owner)
            return
        if isinstance(expr, ast.BinOp):
            self.annotation(expr.left, scope, owner)
            self.annotation(expr.right, scope, owner)
            return
        if isinstance(expr, (ast.List, ast.Tuple)):
            for element in expr.elts:
                self.annotation(element, scope, owner)
            return
        self.visit(expr, scope, owner)

    def quoted(self, node: ast.Constant) -> ast.expr | None:
        """The expression inside a quoted annotation, positioned in the file, when it can be placed exactly."""
        if node.end_lineno != node.lineno:
            return None
        text = self.lines.line_text(node.lineno).encode("utf-8")[node.col_offset : node.end_col_offset].decode("utf-8")
        body = text.lstrip("rRuUbB")
        quote = 3 if body.startswith(('"""', "'''")) else 1
        if "\\" in body or len(body) < 2 * quote + 1:
            return None
        shift = node.col_offset + (len(text) - len(body)) + quote
        try:
            parsed = ast.parse(node.value, mode="eval").body
        except SyntaxError:
            return None
        if "\n" in node.value:
            return None
        for child in ast.walk(parsed):
            if hasattr(child, "lineno"):
                child.lineno = child.end_lineno = node.lineno
                child.col_offset += shift
                child.end_col_offset += shift
        return parsed

    # spans

    def attr_span(self, node: ast.Attribute) -> Span:
        """The span of the identifier after the dot."""
        width = len(node.attr.encode("utf-8"))
        return span(node.end_lineno, node.end_col_offset - width, node.end_lineno, node.end_col_offset)

    def name_span(self, expr: ast.expr) -> Span:
        return self.attr_span(expr) if isinstance(expr, ast.Attribute) else span_of(expr)
