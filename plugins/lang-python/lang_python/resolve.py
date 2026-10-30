"""Names to project declarations: imports, attributes of modules and classes, and the types annotations state."""

import ast
from dataclasses import dataclass

from lang_python.decls import FUNCTION_NODES, Decl, dotted, walk_flat
from lang_python.model import Model, ParsedFile
from lang_python.paths import module_path
from lang_python.project import ModuleLoc
from lang_python.scopes import Scope, binding_scope

MAX_DEPTH = 8
OPTIONAL_WRAPPERS = frozenset({"Optional", "Annotated", "Final", "ClassVar", "Required", "NotRequired", "ReadOnly"})
ALL_MEMBERS = "__all__"
CLASS_RECEIVERS = frozenset({"__new__", "__init_subclass__", "__class_getitem__"})


@dataclass(frozen=True)
class ModuleVal:
    """A module: a project module when `loc` is set, else one outside the project."""

    dotted: str
    loc: ModuleLoc | None


@dataclass(frozen=True)
class DeclVal:
    decl: Decl


@dataclass(frozen=True)
class InstanceVal:
    """A value known to be an instance of a project class."""

    cls: Decl


Value = ModuleVal | DeclVal | InstanceVal


@dataclass(frozen=True)
class ImportModule:
    """`import a.b` binds `a`; `import a.b as x` binds `x` to `a.b`."""

    dotted: str


@dataclass(frozen=True)
class ImportName:
    """`from m import n`: a name of a module, found as a declaration, a re-export or a submodule."""

    level: int
    module: str | None
    name: str


Binding = ImportModule | ImportName


def static_all(tree: ast.Module) -> list[ast.Constant] | None:
    """The string constants of the module's `__all__` when it is written out statically, else None."""
    found: list[ast.Constant] | None = None
    for stmt in walk_flat(tree.body):
        value = None
        if isinstance(stmt, ast.Assign) and any(isinstance(t, ast.Name) and t.id == ALL_MEMBERS for t in stmt.targets):
            found, value = [], stmt.value
        elif isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name) and stmt.target.id == ALL_MEMBERS:
            found, value = [], stmt.value
        elif isinstance(stmt, ast.AugAssign) and isinstance(stmt.target, ast.Name) and stmt.target.id == ALL_MEMBERS:
            value = stmt.value
        elif isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call):
            value = all_call(stmt.value)
        if value is not None and found is not None:
            found.extend(constants(value))
    return found


def all_call(call: ast.Call) -> ast.expr | None:
    """The argument of `__all__.extend(...)` or `__all__.append(...)`."""
    func = call.func
    if not (isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name) and func.value.id == ALL_MEMBERS):
        return None
    if func.attr in ("extend", "append") and len(call.args) == 1:
        return call.args[0]
    return None


def constants(value: ast.expr) -> list[ast.Constant]:
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
        if "imports" not in self.parsed.cache:
            body = self.parsed.tree.body if self.parsed.tree is not None else []
            self.parsed.cache["imports"] = import_bindings(body)
        return self.parsed.cache["imports"]

    def scope_imports(self, scope: Scope) -> dict[str, Binding]:
        if scope.kind == "module":
            return self.module_imports()
        if "imports" not in scope.cache:
            body = getattr(scope.node, "body", None)
            scope.cache["imports"] = import_bindings(body) if isinstance(body, list) else {}
        return scope.cache["imports"]

    def star_modules(self) -> list[ModuleLoc]:
        """The project modules the file imports from with `*`."""
        found: list[ModuleLoc] = []
        for stmt in walk_flat(self.parsed.tree.body if self.parsed.tree is not None else []):
            if isinstance(stmt, ast.ImportFrom) and any(a.name == "*" for a in stmt.names):
                loc = self.locate(stmt.level, stmt.module)
                if loc is not None:
                    found.append(loc)
        return found

    def locate(self, level: int, module: str | None) -> ModuleLoc | None:
        if level == 0:
            return self.project.find(module, self.parsed.rel) if module else None
        return self.project.find_relative(self.parsed.rel, level, module)

    def bound(self, binding: Binding, depth: int = 0) -> Value | None:
        if isinstance(binding, ImportModule):
            return ModuleVal(binding.dotted, self.project.find(binding.dotted, self.parsed.rel))
        return self.from_import(binding, depth)

    def from_import(self, binding: ImportName, depth: int = 0) -> Value | None:
        loc = self.locate(binding.level, binding.module)
        if loc is None or depth > MAX_DEPTH:
            return None
        return self.module_attribute(ModuleVal(binding.module or "", loc), binding.name, depth)

    # values

    def module_attribute(self, module: ModuleVal, name: str, depth: int = 0) -> Value | None:
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
        sub = self.submodule(module, name)
        return sub

    def submodule(self, module: ModuleVal, name: str) -> ModuleVal | None:
        loc = module.loc
        if loc is None or not loc.is_package:
            return None
        base = loc.module if loc.module != "." else ""
        for suffix in (".py", ".pyi", "/__init__.py", "/__init__.pyi"):
            path = (base + "/" if base else "") + name + suffix
            if path in self.project.files:
                dotted_name = (module.dotted + "." if module.dotted else "") + name
                return ModuleVal(dotted_name, ModuleLoc(module_path(path), path, suffix.startswith("/")))
        directory = (base + "/" if base else "") + name
        if directory in self.project.dirs:
            dotted_name = (module.dotted + "." if module.dotted else "") + name
            return ModuleVal(dotted_name, ModuleLoc(directory, None, True))
        return None

    def star_value(self, name: str, depth: int) -> Value | None:
        """A name some `from m import *` of the file brings in."""
        for loc in self.star_modules():
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
            binding = self.scope_imports(bind).get(name)
            if binding is not None:
                return self.bound(binding)
            cls = self.local_types(bind).get(name)
            if cls is None:
                return None
            return DeclVal(cls) if name in bind.cache.get("class_params", ()) else InstanceVal(cls)
        return None

    def local_types(self, scope: Scope) -> dict[str, Decl]:
        """The project class each local name of a function is known to hold: annotated parameters and
        names assigned once, from a call of a class or with an annotation."""
        if "types" in scope.cache:
            return scope.cache["types"]
        scope.cache["types"] = types = {}
        node = scope.node
        if isinstance(node, (*FUNCTION_NODES, ast.Lambda)):
            self.parameter_types(node, scope, types)
        if isinstance(node, FUNCTION_NODES):
            self.assigned_types(node.body, scope, types)
        return types

    def parameter_types(self, node: ast.AST, scope: Scope, types: dict[str, Decl]) -> None:
        args = node.args  # type: ignore[attr-defined]
        every = [*args.posonlyargs, *args.args, *args.kwonlyargs]
        for index, arg in enumerate(every):
            cls = self.annotation_class(arg.annotation, scope.parent or scope) if arg.annotation is not None else None
            if cls is not None:
                types[arg.arg] = cls
        owner = self.owner_class(scope)
        if owner is not None and every and isinstance(node, FUNCTION_NODES) and not has_name(node, "staticmethod"):
            types.setdefault(every[0].arg, owner)
            if has_name(node, "classmethod") or node.name in CLASS_RECEIVERS:
                scope.cache["class_params"] = {every[0].arg}

    def owner_class(self, scope: Scope) -> Decl | None:
        """The project class a method's scope sits in."""
        parent = scope.parent
        if parent is None or parent.kind != "class" or self.decls is None:
            return None
        return self.decls.of(parent.node) if parent.node is not None else None

    def assigned_types(self, body: list[ast.stmt], scope: Scope, types: dict[str, Decl]) -> None:
        seen: dict[str, int] = {}
        known: dict[str, Decl] = {}
        for stmt in walk_flat(body):
            if isinstance(stmt, ast.Assign):
                targets, value, annotation = stmt.targets, stmt.value, None
            elif isinstance(stmt, ast.AnnAssign):
                targets, value, annotation = [stmt.target], stmt.value, stmt.annotation
            else:
                continue
            for target in targets:
                if not isinstance(target, ast.Name):
                    continue
                seen[target.id] = seen.get(target.id, 0) + 1
                cls = None
                if annotation is not None:
                    cls = self.annotation_class(annotation, scope)
                if cls is None and value is not None:
                    inferred = self.expr_value(value, scope)
                    cls = inferred.cls if isinstance(inferred, InstanceVal) else None
                if cls is not None:
                    known[target.id] = cls
        for name, cls in known.items():
            if seen[name] == 1 and name not in types:
                types[name] = cls

    def expr_value(self, expr: ast.expr, scope: Scope) -> Value | None:
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
                return self.returned(callee.decl)
        return None

    def returned(self, decl: Decl) -> Value | None:
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
        if isinstance(base, ModuleVal):
            return self.module_attribute(base, name)
        if isinstance(base, DeclVal) and base.decl.kind in ("field", "var", "const"):
            inferred = self.declared_type(base.decl)
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

    def declared_type(self, decl: Decl) -> Decl | None:
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
        if expr is None:
            return None
        value = self.expr_value(expr, scope)
        if isinstance(value, DeclVal) and value.decl.kind in ("type", "interface"):
            return value.decl
        return None

    # classes

    def bases(self, cls: Decl) -> tuple[list[Decl], bool]:
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
            value = resolver.expr_value(expr, scope)
            if isinstance(value, DeclVal) and value.decl.kind in ("type", "interface") and value.decl is not cls:
                found.append(value.decl)
            elif (dotted(expr) or "").rsplit(".", 1)[-1] not in ("object", "ABC", "Protocol", "Generic"):
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
        bases, _ = self.bases(cls)
        for base in bases:
            found = self.member(base, name, seen | {id(cls)})
            if found is not None:
                return found
        return None

    def ancestors(self, cls: Decl, seen: frozenset[int] = frozenset()) -> list[Decl]:
        """The project classes above a class, nearest first."""
        found: list[Decl] = []
        bases, _ = self.bases(cls)
        for base in bases:
            if id(base) in seen:
                continue
            found.append(base)
            found.extend(self.ancestors(base, seen | {id(cls), id(base)}))
        return found

    def open_class(self, cls: Decl) -> bool:
        """Whether a class may have members the project does not declare: some base lies outside it."""
        _, external = self.bases(cls)
        if external:
            return True
        return any(self.bases(base)[1] for base in self.ancestors(cls))


def resolver_of(model: Model, parsed: ParsedFile) -> Resolver:
    if "resolver" not in parsed.cache:
        parsed.cache["resolver"] = Resolver(model, parsed)
    return parsed.cache["resolver"]


def import_bindings(body: list[ast.stmt]) -> dict[str, Binding]:
    """The names the imports of a body bind; a later import of a name replaces an earlier one."""
    found: dict[str, Binding] = {}
    for stmt in walk_flat(body):
        if isinstance(stmt, ast.Import):
            for alias in stmt.names:
                if alias.asname is not None:
                    found[alias.asname] = ImportModule(alias.name)
                else:
                    top = alias.name.split(".")[0]
                    found[top] = ImportModule(top)
        elif isinstance(stmt, ast.ImportFrom):
            for alias in stmt.names:
                if alias.name != "*":
                    found[alias.asname or alias.name] = ImportName(stmt.level, stmt.module, alias.name)
    return found


def has_name(node: ast.AST, last: str) -> bool:
    for decorator in getattr(node, "decorator_list", []):
        target = decorator.func if isinstance(decorator, ast.Call) else decorator
        if (dotted(target) or "").rsplit(".", 1)[-1] == last:
            return True
    return False


def unwrap_annotation(annotation: ast.expr | None) -> ast.expr | None:
    """The expression that names the type of an annotation, wrappers looked through; None when it is not one
    name (a union of two types, a collection)."""
    if annotation is None:
        return None
    if isinstance(annotation, ast.Constant) and isinstance(annotation.value, str):
        try:
            parsed = ast.parse(annotation.value.strip(), mode="eval").body
        except SyntaxError:
            return None
        return unwrap_annotation(parsed)
    if isinstance(annotation, ast.Subscript):
        wrapper = (dotted(annotation.value) or "").rsplit(".", 1)[-1]
        if wrapper in OPTIONAL_WRAPPERS:
            inner = annotation.slice
            if isinstance(inner, ast.Tuple) and inner.elts:
                inner = inner.elts[0]
            return unwrap_annotation(inner)
        if wrapper == "Union":
            inner = annotation.slice
            members = inner.elts if isinstance(inner, ast.Tuple) else [inner]
            kept = [m for m in members if not is_none(m)]
            return unwrap_annotation(kept[0]) if len(kept) == 1 else None
        return None
    if isinstance(annotation, ast.BinOp) and isinstance(annotation.op, ast.BitOr):
        kept = [m for m in (annotation.left, annotation.right) if not is_none(m)]
        return unwrap_annotation(kept[0]) if len(kept) == 1 else None
    if isinstance(annotation, (ast.Name, ast.Attribute)):
        return annotation
    return None


def is_none(expr: ast.expr) -> bool:
    return isinstance(expr, ast.Constant) and expr.value is None
