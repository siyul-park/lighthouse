"""Function summaries: metrics, flow, forwarding, overriding, and the types a signature states from annotations."""

import ast

from lang_python.symbols import FUNCTION_NODES, Decl, dotted, has_decorator
from lang_python.project import DeclVal, Model, ParsedFile, Resolver, Scope, has_name, unwrap_annotation
from lang_python.extent import start_of
from lang_python.visibility import Reach
from lang_python.flow import FlowWalker
from lang_python.sdk.wire import FunctionSummary, Signature, TypeRef
from lang_python.tests import body_statements, manual_assertions

NO_OVERRIDE = frozenset({"__init__", "__new__", "__init_subclass__", "__class_getitem__"})
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


class Functions:
    """Builds the summaries of the functions of one parsed file."""

    def __init__(
        self, model: Model, parsed: ParsedFile, resolver: Resolver, types: Types, module_scope: Scope, test: bool
    ):
        self.model = model
        self.parsed = parsed
        self.resolver = resolver
        self.types = types
        self.module_scope = module_scope
        self.test = test

    def summary(self, decl: Decl, scope: Scope) -> FunctionSummary:
        node = decl.body
        walker = FlowWalker(lambda call: self.calls_self(call, decl, scope))
        walker.run(node.body)  # type: ignore[union-attr]
        statements = [] if isinstance(node, ast.Lambda) else body_statements(node)
        params = own_parameters(node, decl)
        summary: FunctionSummary = {
            "symbol": decl.id,
            "max_nesting": walker.deepest,
            "statements": walker.count,
            "top_level": 1 if isinstance(node, ast.Lambda) else len(statements),
            "params": len(params),
            "returns": returns(node),
            "tokens": self.token_count(node, statements),
            "flow": walker.events,
        }
        forwards = self.forward_target(decl, node, scope)
        if forwards is not None:
            summary["forwards_to"] = forwards
        signature = self.types.signature(decl, self.resolver, self.module_scope)
        if signature is not None:
            summary["signature"] = signature
        if self.implementation(decl):
            summary["implementation"] = True
        if self.types.constructs(decl, self.resolver, self.module_scope):
            summary["constructs"] = True
        if self.test:
            manual = manual_assertions(node)
            if manual > 0:
                summary["manual_assertions"] = manual
        return summary

    def token_count(self, node: ast.AST, statements: list[ast.stmt]) -> int:
        tokens = self.parsed.tokens
        if isinstance(node, ast.Lambda):
            return tokens.count(
                (node.body.lineno, node.body.col_offset), (node.body.end_lineno, node.body.end_col_offset)
            )
        if not statements:
            return 0
        line, col = start_of(statements[0], self.parsed.lines)
        last = statements[-1]
        return tokens.count((line, col), (last.end_lineno, last.end_col_offset))

    def calls_self(self, call: ast.Call, decl: Decl, scope: Scope) -> bool:
        value = self.resolver.expr_value(call.func, scope)
        return isinstance(value, DeclVal) and value.decl is decl

    def implementation(self, decl: Decl) -> bool:
        """Whether a method overrides one of a project base class, an interface or an abstract class, or says so."""
        node = decl.body
        if decl.kind != "method" or decl.owner is None or decl.name in NO_OVERRIDE or node is None:
            return False
        if has_name(node, "override"):
            return True
        for base in self.resolver.ancestors(decl.owner):
            target = self.model.decls(self.resolver.file_of(base))
            if target is not None and target.member(base, decl.name) is not None:
                return True
        return False

    def forward_target(self, decl: Decl, node: ast.AST, scope: Scope) -> str | None:
        """The call a body consists of when it passes every parameter on, in order: `return self.read(key)`."""
        if isinstance(node, ast.Lambda):
            return None
        statements = body_statements(node)
        if len(statements) != 1:
            return None
        only = statements[0]
        expr = only.value if isinstance(only, (ast.Return, ast.Expr)) else None
        while isinstance(expr, ast.Await):
            expr = expr.value
        if not isinstance(expr, ast.Call):
            return None
        params = own_parameters(node, decl)
        if not passes_on(node, params, expr):
            return None
        callee = expr.func
        method = decl.owner is not None
        everything = [*node.args.posonlyargs, *node.args.args]  # type: ignore[attr-defined]
        if isinstance(callee, ast.Name) and not method:
            pass
        elif (
            isinstance(callee, ast.Attribute)
            and method
            and everything
            and isinstance(callee.value, ast.Name)
            and callee.value.id == everything[0].arg
        ):
            pass
        else:
            return None
        value = self.resolver.expr_value(callee, scope)
        return value.decl.id if isinstance(value, DeclVal) else None


def passes_on(node: ast.AST, params: list[ast.arg], call: ast.Call) -> bool:
    """Whether a call's arguments are exactly the parameters, named and in order."""
    args = node.args  # type: ignore[attr-defined]
    positional = [*args.posonlyargs, *args.args]
    wanted = [p.arg for p in params if any(p is q for q in positional)]
    if args.vararg is not None:
        wanted.append("*" + args.vararg.arg)
    given: list[str] = []
    for argument in call.args:
        if isinstance(argument, ast.Starred) and isinstance(argument.value, ast.Name):
            given.append("*" + argument.value.id)
        elif isinstance(argument, ast.Name):
            given.append(argument.id)
        else:
            return False
    if given != wanted:
        return False
    keywords: dict[str, str] = {}
    for keyword in call.keywords:
        if not isinstance(keyword.value, ast.Name):
            return False
        keywords[keyword.arg or "**"] = keyword.value.id if keyword.arg is not None else "**" + keyword.value.id
    expected = {a.arg: a.arg for a in args.kwonlyargs}
    if args.kwarg is not None:
        expected["**"] = "**" + args.kwarg.arg
    return keywords == expected
