"""Names to project declarations: imports, attributes of modules and classes, and the types annotations state."""

import ast
import posixpath
from dataclasses import dataclass
from typing import Iterator

from lang_python.symbols import FUNCTION_NODES, Decl, dotted, walk_flat
from lang_python.project.files import ModuleLoc
from lang_python.project.model import Model, ParsedFile
from lang_python.modules import module_path
from lang_python.project.scopes import Scope, binding_scope

MAX_DEPTH = 8
OPTIONAL_WRAPPERS = frozenset({"Optional", "Annotated", "Final", "ClassVar", "Required", "NotRequired", "ReadOnly"})
ALL_MEMBERS = "__all__"
CLASS_RECEIVERS = frozenset({"__new__", "__init_subclass__", "__class_getitem__"})
SUBMODULE_SUFFIXES = (".py", ".pyi", "/__init__.py", "/__init__.pyi")
PLAIN_BASES = frozenset({"object", "ABC", "Protocol", "Generic"})


@dataclass(frozen=True)
class ModuleVal:
    """A module: a project module when `loc` is set, else one outside the project."""

    dotted: str
    loc: ModuleLoc | None


@dataclass(frozen=True)
class DeclVal:
    """A name that stands for a declaration of the project."""

    decl: Decl


@dataclass(frozen=True)
class InstanceVal:
    """A value known to be an instance of a project class."""

    cls: Decl


Value = ModuleVal | DeclVal | InstanceVal


@dataclass(frozen=True)
class _ImportModule:
    """`import a.b` binds `a`; `import a.b as x` binds `x` to `a.b`."""

    dotted: str


@dataclass(frozen=True)
class _ImportName:
    """`from m import n`: a name of a module, found as a declaration, a re-export or a submodule."""

    level: int
    module: str | None
    name: str


Binding = _ImportModule | _ImportName


def _names_all(target: ast.expr) -> bool:
    """Whether an assignment target is the name `__all__`."""
    return isinstance(target, ast.Name) and target.id == ALL_MEMBERS


def _assigns_all(stmt: ast.Assign | ast.AnnAssign) -> bool:
    """Whether an assignment, plain or annotated, writes `__all__`."""
    targets = stmt.targets if isinstance(stmt, ast.Assign) else [stmt.target]
    return any(_names_all(t) for t in targets)


def _all_update(stmt: ast.stmt) -> tuple[bool, ast.expr | None]:
    """How a statement changes `__all__`: whether it starts the list afresh, and the value it adds."""
    if isinstance(stmt, (ast.Assign, ast.AnnAssign)):
        return (True, stmt.value) if _assigns_all(stmt) else (False, None)
    if isinstance(stmt, ast.AugAssign) and _names_all(stmt.target):
        return False, stmt.value
    if isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call):
        return False, _all_call(stmt.value)
    return False, None


def static_all(tree: ast.Module) -> list[ast.Constant] | None:
    """The string constants of the module's `__all__` when it is written out statically, else None."""
    found: list[ast.Constant] | None = None
    for stmt in walk_flat(tree.body):
        fresh, value = _all_update(stmt)
        if fresh:
            found = []
        if value is not None and found is not None:
            found.extend(constants(value))
    return found


def _all_call(call: ast.Call) -> ast.expr | None:
    """The argument of `__all__.extend(...)` or `__all__.append(...)`."""
    func = call.func
    if not (isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name) and func.value.id == ALL_MEMBERS):
        return None
    if func.attr in ("extend", "append") and len(call.args) == 1:
        return call.args[0]
    return None


def constants(value: ast.expr) -> list[ast.Constant]:
    """The string constants a value spells out: a list, tuple or set of them, one string, or a sum of those."""
    if isinstance(value, (ast.List, ast.Tuple, ast.Set)):
        return [e for e in value.elts if isinstance(e, ast.Constant) and isinstance(e.value, str)]
    if isinstance(value, ast.Constant) and isinstance(value.value, str):
        return [value]
    if isinstance(value, ast.BinOp) and isinstance(value.op, ast.Add):
        return constants(value.left) + constants(value.right)
    return []


class Resolver:
    """Resolves the names of one file."""

    def __init__(self, model: Model, parsed: ParsedFile):
        self.model = model
        self.parsed = parsed
        self.project = model.project
        self.decls = parsed.decls

    # imports

    def module_imports(self) -> dict[str, Binding]:
        """The names the module-level imports of the file bind, each with how it was imported."""
        if "imports" not in self.parsed.cache:
            body = self.parsed.tree.body if self.parsed.tree is not None else []
            self.parsed.cache["imports"] = _import_bindings(body)
        return self.parsed.cache["imports"]

    def _scope_imports(self, scope: Scope) -> dict[str, Binding]:
        if scope.kind == "module":
            return self.module_imports()
        if "imports" not in scope.cache:
            body = getattr(scope.node, "body", None)
            scope.cache["imports"] = _import_bindings(body) if isinstance(body, list) else {}
        return scope.cache["imports"]

    def _star_modules(self) -> list[ModuleLoc]:
        """The project modules the file imports from with `*`."""
        found: list[ModuleLoc] = []
        for stmt in walk_flat(self.parsed.tree.body if self.parsed.tree is not None else []):
            if isinstance(stmt, ast.ImportFrom) and any(a.name == "*" for a in stmt.names):
                loc = self._locate(stmt.level, stmt.module)
                if loc is not None:
                    found.append(loc)
        return found

    def _locate(self, level: int, module: str | None) -> ModuleLoc | None:
        if level == 0:
            return self.project.find(module, self.parsed.rel) if module else None
        return self.project.find_relative(self.parsed.rel, level, module)

    def bound(self, binding: Binding, depth: int = 0) -> Value | None:
        """What an import binding stands for: a module, or a name found in the module it comes from."""
        if isinstance(binding, _ImportModule):
            return ModuleVal(binding.dotted, self.project.find(binding.dotted, self.parsed.rel))
        return self._from_import(binding, depth)

    def _from_import(self, binding: _ImportName, depth: int = 0) -> Value | None:
        loc = self._locate(binding.level, binding.module)
        if loc is None or depth > MAX_DEPTH:
            return None
        return self.module_attribute(ModuleVal(binding.module or "", loc), binding.name, depth)

    # values

    def module_attribute(self, module: ModuleVal, name: str, depth: int = 0) -> Value | None:
        """What a name of a module is: a declaration, a re-export, a name a star import brings in or a submodule."""
        loc = module.loc
        if loc is None or depth > MAX_DEPTH:
            return None
        if loc.rel is not None:
            target = self.model.file(loc.rel)
            if target is not None and target.decls is not None:
                found = target.decls.top.get(name)
                if found is not None:
                    return DeclVal(found)
                binding = Resolver(self.model, target).module_imports().get(name)
                if binding is not None:
                    return Resolver(self.model, target).bound(binding, depth + 1)
                star = Resolver(self.model, target).star_value(name, depth + 1)
                if star is not None:
                    return star
        sub = self._submodule(module, name)
        return sub

    def _submodule(self, module: ModuleVal, name: str) -> ModuleVal | None:
        loc = module.loc
        if loc is None or not loc.is_package:
            return None
        base = loc.module if loc.module != "." else ""
        dotted_name = f"{module.dotted}.{name}" if module.dotted else name
        for suffix in SUBMODULE_SUFFIXES:
            path = posixpath.join(base, name) + suffix
            if path in self.project.files:
                return ModuleVal(dotted_name, ModuleLoc(module_path(path), path, suffix.startswith("/")))
        directory = posixpath.join(base, name)
        if directory in self.project.dirs:
            return ModuleVal(dotted_name, ModuleLoc(directory, None, True))
        return None

    def star_value(self, name: str, depth: int) -> Value | None:
        """A name some `from m import *` of the file brings in."""
        for loc in self._star_modules():
            if loc.rel is None:
                continue
            target = self.model.file(loc.rel)
            if target is None or target.tree is None:
                continue
            exported = static_all(target.tree)
            if exported is not None and name not in {c.value for c in exported}:
                continue
            if exported is None and name.startswith("_"):
                continue
            found = self.module_attribute(ModuleVal("", loc), name, depth + 1)
            if found is not None:
                return found
        return None

    def module_value(self, name: str) -> Value | None:
        """What a module-level name of the file stands for, through declarations, imports and star imports."""
        if self.decls is not None and name in self.decls.top:
            return DeclVal(self.decls.top[name])
        binding = self.module_imports().get(name)
        if binding is not None:
            return self.bound(binding)
        return self.star_value(name, 0)

    def lookup(self, name: str, scope: Scope) -> Value | None:
        """What a bare name stands for: an import, a declaration, an instance a local is known to hold."""
        bind = binding_scope(scope, name)
        if bind is None:
            return self.module_value(name)
        if bind.kind == "function":
            binding = self._scope_imports(bind).get(name)
            if binding is not None:
                return self.bound(binding)
            cls = self._local_types(bind).get(name)
            if cls is None:
                return None
            return DeclVal(cls) if name in bind.cache.get("class_params", ()) else InstanceVal(cls)
        return None

    def _local_types(self, scope: Scope) -> dict[str, Decl]:
        """The project class each local name of a function is known to hold: annotated parameters and
        names assigned once, from a call of a class or with an annotation."""
        if "types" in scope.cache:
            return scope.cache["types"]
        scope.cache["types"] = types = {}
        node = scope.node
        if isinstance(node, (*FUNCTION_NODES, ast.Lambda)):
            self._parameter_types(node, scope, types)
        if isinstance(node, FUNCTION_NODES):
            self._assigned_types(node.body, scope, types)
        return types

    def _parameter_types(self, node: ast.AST, scope: Scope, types: dict[str, Decl]) -> None:
        args = node.args  # type: ignore[attr-defined]
        every = [*args.posonlyargs, *args.args, *args.kwonlyargs]
        for arg in every:
            cls = self.annotation_class(arg.annotation, scope.parent or scope)
            if cls is not None:
                types[arg.arg] = cls
        self._receiver_type(node, scope, every, types)

    def _receiver_type(self, node: ast.AST, scope: Scope, every: list[ast.arg], types: dict[str, Decl]) -> None:
        """A method's first parameter holds its class, or the class itself for a classmethod."""
        owner = self._owner_class(scope)
        if owner is None or not every or not isinstance(node, FUNCTION_NODES) or has_name(node, "staticmethod"):
            return
        types.setdefault(every[0].arg, owner)
        if has_name(node, "classmethod") or node.name in CLASS_RECEIVERS:
            scope.cache["class_params"] = {every[0].arg}

    def _owner_class(self, scope: Scope) -> Decl | None:
        """The project class a method's scope sits in."""
        parent = scope.parent
        if parent is None or parent.kind != "class" or self.decls is None:
            return None
        return self.decls.of(parent.node) if parent.node is not None else None

    def _assigned_types(self, body: list[ast.stmt], scope: Scope, types: dict[str, Decl]) -> None:
        seen: dict[str, int] = {}
        known: dict[str, Decl] = {}
        for name, value, annotation in _plain_assignments(body):
            seen[name] = seen.get(name, 0) + 1
            cls = self._assigned_class(value, annotation, scope)
            if cls is not None:
                known[name] = cls
        for name, cls in known.items():
            if seen[name] == 1 and name not in types:
                types[name] = cls

    def _assigned_class(self, value: ast.expr | None, annotation: ast.expr | None, scope: Scope) -> Decl | None:
        """The project class an assignment gives a name: its annotation, else what its value is an instance of."""
        cls = self.annotation_class(annotation, scope)
        if cls is None and value is not None:
            inferred = self.expr_value(value, scope)
            cls = inferred.cls if isinstance(inferred, InstanceVal) else None
        return cls

    def expr_value(self, expr: ast.expr, scope: Scope) -> Value | None:
        """What an expression stands for when scopes, imports and annotations settle it; None otherwise."""
        if isinstance(expr, ast.Name):
            return self.lookup(expr.id, scope)
        if isinstance(expr, ast.Attribute):
            base = self.expr_value(expr.value, scope)
            return None if base is None else self.attribute(base, expr.attr)
        if isinstance(expr, ast.Call):
            callee = self.expr_value(expr.func, scope)
            if isinstance(callee, DeclVal) and callee.decl.kind in ("type", "interface"):
                return InstanceVal(callee.decl)
            if isinstance(callee, DeclVal) and callee.decl.kind in ("function", "method"):
                return self._returned(callee.decl)
        return None

    def _returned(self, decl: Decl) -> Value | None:
        """The instance a function's return annotation states."""
        node = decl.body if isinstance(decl.body, FUNCTION_NODES) else None
        if node is None or node.returns is None:
            return None
        target = self.model.file(self.file_of(decl))
        if target is None or target.decls is None:
            return None
        scope = target.scopes().module(target.tree)
        cls = Resolver(self.model, target).annotation_class(node.returns, scope)
        return None if cls is None else InstanceVal(cls)

    def file_of(self, decl: Decl) -> str:
        """The file a declaration lives in."""
        for rel in (
            decl.module + ".py",
            decl.module + ".pyi",
            decl.module + "/__init__.py",
            decl.module + "/__init__.pyi",
        ):
            if rel in self.project.files:
                return rel
        return self.parsed.rel

    def attribute(self, base: Value, name: str) -> Value | None:
        """What `base.name` stands for: a name of a module, or a member of a class or of an instance of one."""
        if isinstance(base, ModuleVal):
            return self.module_attribute(base, name)
        if isinstance(base, DeclVal) and base.decl.kind in ("field", "var", "const"):
            inferred = self._declared_type(base.decl)
            if inferred is None:
                return None
            base = InstanceVal(inferred)
        if isinstance(base, DeclVal):
            if base.decl.kind not in ("type", "interface"):
                return None
            found = self.member(base.decl, name)
        else:
            found = self.member(base.cls, name)
        return None if found is None else DeclVal(found)

    def _declared_type(self, decl: Decl) -> Decl | None:
        """The project class a field or variable is annotated with, or assigned a call of."""
        target = self.model.file(self.file_of(decl))
        if target is None or target.decls is None:
            return None
        resolver = Resolver(self.model, target)
        scope = target.scopes().module(target.tree)
        if decl.annotation is not None:
            return resolver.annotation_class(decl.annotation, scope)
        node = decl.node
        value = getattr(node, "value", None)
        if isinstance(value, ast.Call):
            inferred = resolver.expr_value(value, scope)
            return inferred.cls if isinstance(inferred, InstanceVal) else None
        return None

    def annotation_class(self, annotation: ast.expr | None, scope: Scope) -> Decl | None:
        """The project class an annotation names, through `Optional`, `X | None`, `Annotated` and quotes."""
        expr = unwrap_annotation(annotation)
        return None if expr is None else self._class_of(expr, scope)

    def _class_of(self, expr: ast.expr, scope: Scope) -> Decl | None:
        """The project class an expression names."""
        value = self.expr_value(expr, scope)
        if isinstance(value, DeclVal) and value.decl.kind in ("type", "interface"):
            return value.decl
        return None

    # classes

    def _bases(self, cls: Decl) -> tuple[list[Decl], bool]:
        """The project classes a class derives from, and whether some base lies outside the project."""
        target = self.model.file(self.file_of(cls))
        node = cls.node
        if target is None or target.decls is None or not isinstance(node, ast.ClassDef):
            return [], False
        resolver = Resolver(self.model, target)
        scope = target.scopes().module(target.tree)
        found: list[Decl] = []
        external = False
        for base in node.bases:
            expr = base.value if isinstance(base, ast.Subscript) else base
            decl = resolver._class_of(expr, scope)
            if decl is not None and decl is not cls:
                found.append(decl)
            elif (dotted(expr) or "").rsplit(".", 1)[-1] not in PLAIN_BASES:
                external = True
        return found, external

    def member(self, cls: Decl, name: str, seen: frozenset[int] = frozenset()) -> Decl | None:
        """A member of a class or of the project classes it derives from."""
        if id(cls) in seen:
            return None
        target = self.model.file(self.file_of(cls))
        if target is not None and target.decls is not None:
            found = target.decls.member(cls, name)
            if found is not None:
                return found
        bases, _ = self._bases(cls)
        for base in bases:
            found = self.member(base, name, seen | {id(cls)})
            if found is not None:
                return found
        return None

    def ancestors(self, cls: Decl, seen: frozenset[int] = frozenset()) -> list[Decl]:
        """The project classes above a class, nearest first."""
        found: list[Decl] = []
        bases, _ = self._bases(cls)
        for base in bases:
            if id(base) in seen:
                continue
            found.append(base)
            found.extend(self.ancestors(base, seen | {id(cls), id(base)}))
        return found


def _plain_assignments(body: list[ast.stmt]) -> Iterator[tuple[str, ast.expr | None, ast.expr | None]]:
    """The names a body assigns, each with the value and the annotation of its statement."""
    for stmt in walk_flat(body):
        if isinstance(stmt, ast.Assign):
            targets, value, annotation = stmt.targets, stmt.value, None
        elif isinstance(stmt, ast.AnnAssign):
            targets, value, annotation = [stmt.target], stmt.value, stmt.annotation
        else:
            continue
        for target in targets:
            if isinstance(target, ast.Name):
                yield target.id, value, annotation


def resolver_of(model: Model, parsed: ParsedFile) -> Resolver:
    """The resolver of a parsed file, made once."""
    if "resolver" not in parsed.cache:
        parsed.cache["resolver"] = Resolver(model, parsed)
    return parsed.cache["resolver"]


def _import_bindings(body: list[ast.stmt]) -> dict[str, Binding]:
    """The names the imports of a body bind; a later import of a name replaces an earlier one."""
    found: dict[str, Binding] = {}
    for stmt in walk_flat(body):
        if isinstance(stmt, ast.Import):
            for alias in stmt.names:
                if alias.asname is not None:
                    found[alias.asname] = _ImportModule(alias.name)
                else:
                    top = alias.name.split(".")[0]
                    found[top] = _ImportModule(top)
        elif isinstance(stmt, ast.ImportFrom):
            for alias in stmt.names:
                if alias.name != "*":
                    found[alias.asname or alias.name] = _ImportName(stmt.level, stmt.module, alias.name)
    return found


def has_name(node: ast.AST, last: str) -> bool:
    """Whether a def or class has a decorator whose last dotted name is `last`."""
    for decorator in getattr(node, "decorator_list", []):
        target = decorator.func if isinstance(decorator, ast.Call) else decorator
        if (dotted(target) or "").rsplit(".", 1)[-1] == last:
            return True
    return False


def unwrap_annotation(annotation: ast.expr | None) -> ast.expr | None:
    """The expression that names the type of an annotation, wrappers looked through; None when it is not one
    name (a union of two types, a collection)."""
    if isinstance(annotation, ast.Constant):
        return _unwrap_quoted(annotation)
    if isinstance(annotation, ast.Subscript):
        return _unwrap_subscript(annotation)
    if isinstance(annotation, ast.BinOp) and isinstance(annotation.op, ast.BitOr):
        return _only_type([annotation.left, annotation.right])
    if isinstance(annotation, (ast.Name, ast.Attribute)):
        return annotation
    return None


def _unwrap_quoted(annotation: ast.Constant) -> ast.expr | None:
    """The type a quoted annotation names, read as the expression it holds."""
    if not isinstance(annotation.value, str):
        return None
    try:
        parsed = ast.parse(annotation.value.strip(), mode="eval").body
    except SyntaxError:
        return None
    return unwrap_annotation(parsed)


def _unwrap_subscript(annotation: ast.Subscript) -> ast.expr | None:
    """The type inside `Optional[X]`, `Annotated[X, ...]` and the like, or the one non-None member of a `Union`."""
    wrapper = (dotted(annotation.value) or "").rsplit(".", 1)[-1]
    inner = annotation.slice
    if wrapper in OPTIONAL_WRAPPERS:
        return unwrap_annotation(inner.elts[0] if isinstance(inner, ast.Tuple) and inner.elts else inner)
    if wrapper == "Union":
        return _only_type(inner.elts if isinstance(inner, ast.Tuple) else [inner])
    return None


def _only_type(members: list[ast.expr]) -> ast.expr | None:
    """The type of the one member that is not None; None when several are."""
    kept = [m for m in members if not _is_none(m)]
    return unwrap_annotation(kept[0]) if len(kept) == 1 else None


def _is_none(expr: ast.expr) -> bool:
    return isinstance(expr, ast.Constant) and expr.value is None
