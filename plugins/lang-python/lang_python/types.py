"""The types a declaration states, as the protocol writes them: from annotations only."""

import ast

from lang_python.decls import FUNCTION_NODES, Decl, dotted, has_decorator
from lang_python.resolve import Resolver, unwrap_annotation
from lang_python.scopes import Scope
from lang_python.visibility import Reach
from lang_python.wire import Signature, TypeRef

NOTHING = frozenset({"None", "NoReturn", "Never"})
UNTYPED = "Any"


def text_of(annotation: ast.expr) -> str:
    """The annotation as written, normalized: a quoted annotation is shown without its quotes."""
    if isinstance(annotation, ast.Constant) and isinstance(annotation.value, str):
        return annotation.value.strip()
    return ast.unparse(annotation)


def is_nothing(annotation: ast.expr) -> bool:
    name = text_of(annotation).rsplit(".", 1)[-1]
    return name in NOTHING


def receiver_free(node: ast.FunctionDef | ast.AsyncFunctionDef, decl: Decl) -> bool:
    """Whether a def has no receiver to leave out: a function, or a static method."""
    return decl.owner is None or has_decorator(node, "staticmethod")


def parameters(node: ast.AST) -> list[ast.arg]:
    args = node.args  # type: ignore[attr-defined]
    return [*args.posonlyargs, *args.args, *([args.vararg] if args.vararg else []), *args.kwonlyargs] + (
        [args.kwarg] if args.kwarg else []
    )


def own_parameters(node: ast.AST, decl: Decl | None) -> list[ast.arg]:
    """The declared parameters, the receiver excluded."""
    found = parameters(node)
    if decl is not None and isinstance(node, FUNCTION_NODES) and not receiver_free(node, decl) and found:
        return found[1:]
    return found


class Types:
    """Builds `TypeRef`s and signatures."""

    def __init__(self, reach: Reach):
        self.reach = reach

    def ref(self, annotation: ast.expr | None, resolver: Resolver, scope: Scope) -> TypeRef:
        if annotation is None:
            return {"text": UNTYPED}
        ref: TypeRef = {"text": text_of(annotation)}
        cls = resolver.annotation_class(annotation, scope)
        if cls is not None:
            ref["symbol"] = cls.id[: cls.id.rindex("#")]
            ref["exported"] = self.reach.visibility(cls, resolver.file_of(cls)) != "private"
        return ref

    def field(self, decl: Decl, resolver: Resolver, scope: Scope) -> tuple[TypeRef, bool] | None:
        if decl.annotation is None:
            return None
        optional = nullable(decl.annotation) or (
            decl.default and "dataclass" in (decl.owner.traits if decl.owner else ())
        )
        return self.ref(decl.annotation, resolver, scope), bool(optional)

    def signature(self, decl: Decl, resolver: Resolver, scope: Scope) -> Signature | None:
        node = decl.body
        if node is None:
            return None
        params = [self.ref(arg.annotation, resolver, scope) for arg in own_parameters(node, decl)]
        results: list[TypeRef] = []
        if returns(node) == 1:
            annotation = node.returns if isinstance(node, FUNCTION_NODES) else None
            results.append(self.ref(annotation, resolver, scope))
        if not params and not results:
            return None
        return {"params": params, "results": results}

    def constructs(self, decl: Decl, resolver: Resolver, scope: Scope) -> bool:
        """Whether a function builds a value of a class: `__init__`, or a def returning its own class."""
        node = decl.body
        if not isinstance(node, FUNCTION_NODES):
            return False
        if decl.name == "__init__":
            return True
        if decl.owner is not None and not (has_decorator(node, "classmethod") or has_decorator(node, "staticmethod")):
            return False
        annotation = unwrap_annotation(node.returns)
        if annotation is None:
            return False
        if decl.owner is not None and text_of(annotation) in ("Self", "typing.Self", decl.owner.name):
            return True
        cls = resolver.annotation_class(node.returns, scope)
        return cls is not None and cls.module == decl.module


def nullable(annotation: ast.expr) -> bool:
    """Whether an annotation admits None: `Optional[X]`, `X | None`, `Union[X, None]`."""
    if isinstance(annotation, ast.Constant):
        if isinstance(annotation.value, str):
            try:
                return nullable(ast.parse(annotation.value.strip(), mode="eval").body)
            except SyntaxError:
                return False
        return annotation.value is None
    if isinstance(annotation, ast.BinOp) and isinstance(annotation.op, ast.BitOr):
        return nullable(annotation.left) or nullable(annotation.right)
    if isinstance(annotation, ast.Subscript):
        wrapper = (dotted(annotation.value) or "").rsplit(".", 1)[-1]
        if wrapper == "Optional":
            return True
        if wrapper == "Union" and isinstance(annotation.slice, ast.Tuple):
            return any(nullable(e) for e in annotation.slice.elts)
    return False


def returns(node: ast.AST) -> int:
    """The number of results: one for a value, none for `None`; unannotated, by the returns of the body."""
    if isinstance(node, ast.Lambda):
        return 1
    annotation = node.returns  # type: ignore[attr-defined]
    if annotation is not None:
        return 0 if is_nothing(annotation) else 1
    return 1 if yields_value(node.body) else 0  # type: ignore[attr-defined]


def yields_value(body: list[ast.stmt]) -> bool:
    """Whether a body returns a value or yields, not counting what nested functions do."""
    stack: list[ast.AST] = list(body)
    while stack:
        node = stack.pop()
        if isinstance(node, ast.Return) and node.value is not None:
            if not (isinstance(node.value, ast.Constant) and node.value.value is None):
                return True
        if isinstance(node, (ast.Yield, ast.YieldFrom)):
            return True
        if isinstance(node, (*FUNCTION_NODES, ast.Lambda, ast.ClassDef)):
            continue
        stack.extend(ast.iter_child_nodes(node))
    return False
